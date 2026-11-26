//! The IRC connections zircd makes, one per organisation, from the settings in
//! `config.toml`. The file's format lives in [`zirc_core::settings`], shared with
//! `zirc init`.

use std::path::Path;
use zirc_proto::PROGRAM;

use anyhow::{Context as _, Result};
use zirc_core::names;
use zirc_core::settings::{self, Organisation, Settings};
use zirc_irc::{Config, Credentials};

/// The settings in `path`.
pub fn read(path: &Path) -> Result<Settings> {
    let text = std::fs::read_to_string(path).with_context(|| {
        format!(
            "reading {}: run `zirc init` to say which server to use",
            path.display()
        )
    })?;
    Settings::from_toml(&text).with_context(|| format!("reading {}", path.display()))
}

/// `config.toml` as a running zircd reads it: parsed once, and again only when the file has
/// changed, so an edit counts without a restart and a post or a lease doesn't parse it anew.
/// Every getter here once read and parsed the file for itself, a dozen times at start and
/// again for every post.
#[derive(Debug)]
pub struct File {
    path: std::path::PathBuf,
    read: std::sync::Mutex<Read>,
}

/// What [`File`] last read: the settings, if they could be, and the file as it was then.
#[derive(Debug, Default)]
struct Read {
    stamp: Option<(std::time::SystemTime, u64)>,
    settings: Option<Settings>,
}

impl File {
    /// `config.toml` at `path`, not read until it's asked.
    pub fn at(path: &Path) -> Self {
        Self {
            path: path.to_owned(),
            read: std::sync::Mutex::default(),
        }
    }

    /// What `ask` makes of the settings as the file now says them; `None` when it can't be
    /// read, as while it's being written or once the machine is signed out.
    pub fn ask<T>(&self, ask: impl FnOnce(&Settings) -> Option<T>) -> Option<T> {
        let mut read = self
            .read
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Its time and size, which one cheap look says: only a change is worth a parse.
        let stamp = std::fs::metadata(&self.path)
            .ok()
            .and_then(|meta| Some((meta.modified().ok()?, meta.len())));
        if stamp.is_none() || stamp != read.stamp {
            read.settings = stamp.and_then(|_| self::read(&self.path).ok());
            read.stamp = stamp;
        }
        read.settings.as_ref().and_then(ask)
    }
}

/// The names of the organisations `settings`, read from `path`, signs this machine in to,
/// the first first.
pub fn organisations(settings: &Settings, path: &Path) -> Result<Vec<String>> {
    if let Some(name) = settings
        .organisations()
        .iter()
        .map(Organisation::name)
        .find(|name| !settings::valid_org_name(name))
    {
        anyhow::bail!(
            "{} names an organisation {name:?}, which {PROGRAM} can't keep a directory for: give \
             it a name without slashes that doesn't start with a dot",
            path.display()
        );
    }
    if let Some(name) = settings.duplicate_organisation() {
        anyhow::bail!(
            "{} has two organisations called {name}: give one a name of its own",
            path.display()
        );
    }
    Ok(settings
        .organisations()
        .iter()
        .map(Organisation::name)
        .collect())
}

/// The IRC connection settings in `path` for the organisation `org`: when `first`, the
/// first organisation, even if its name changed with its server.
pub fn load(path: &Path, org: &str, first: bool) -> Result<Config> {
    load_in(&read(path)?, path, org, first)
}

/// [`load`], from `settings` as read from `path`.
pub fn load_in(settings: &Settings, path: &Path, org: &str, first: bool) -> Result<Config> {
    let org = find(settings, org, first)
        .with_context(|| format!("{}: this machine isn't signed in to {org}", path.display()))?;
    let home = path.parent().unwrap_or(Path::new("."));
    Ok(connection(
        settings,
        org,
        &zirc_core::paths::device_key(home),
    ))
}

/// The organisation `org` in `settings`: when `first`, the first, even if its name changed
/// with its server, as a machine in one organisation whose server moved finds it.
pub fn find(settings: &Settings, org: &str, first: bool) -> Option<Organisation> {
    let orgs = settings.organisations();
    orgs.iter()
        .find(|it| it.name() == org)
        .or_else(|| first.then(|| &orgs[0]))
        .cloned()
}

/// Whether zircd updates itself: unless `config.toml` says `auto_update = false`.
pub fn auto_update(settings: &Settings) -> bool {
    settings.auto_update.unwrap_or(true)
}

/// Whether this machine logged in through zirc.dev (`config.toml` names the web app), whose
/// servers' accounts only zirc.dev makes: a person's machines are `<handle>-<machine>` there,
/// so an account says whose machine it is ([`zirc_core::agents::may_query`]).
pub fn zirc_dev(settings: &Settings, org: &str) -> bool {
    web(settings, org).is_some_and(|web| zirc_core::agents::is_zirc_dev(&web))
}

/// The web app `org` signed in through, as `settings` names it, such as `https://zirc.dev`.
pub fn web(settings: &Settings, org: &str) -> Option<String> {
    settings.organisation(org).and_then(|org| org.web)
}

/// How often at most zircd tells a channel what this machine is working on, as `settings`
/// sets it: five minutes unless set. Each time is a line per worktree with changes, kept in the
/// channel's history, and a machine with many sessions changes something every few minutes:
/// at a minute, it was most of a busy channel's history. Overlaps between this machine's own
/// worktrees don't wait for it.
pub fn announce_every(settings: &Settings) -> std::time::Duration {
    std::time::Duration::from_secs(settings.announce_every.unwrap_or(300))
}

/// What this machine must have room for before its sessions ask for lease `name`, as
/// `settings` says (`[leases.<name>]`): asked of [`File`], so an edit counts without a restart.
pub fn lease_room(settings: &Settings, name: &str) -> Option<zirc_core::settings::LeaseRoom> {
    settings.leases.get(name).cloned()
}

/// The breaker's limits on wakes, as `settings` sets them (`[wakes]`): the defaults for each left
/// out. Read at start.
pub fn wake_limits(settings: &Settings) -> (usize, usize) {
    let set = settings.wakes.clone().unwrap_or_default();
    (
        set.per_session
            .unwrap_or(crate::daemon::breaker::PER_SESSION)
            .max(1),
        set.machine
            .unwrap_or(crate::daemon::breaker::MACHINE)
            .max(1),
    )
}

/// Whether a request asked with a recommendation is taken as recommended at once
/// (`accept_recommended`): only when `settings` says so.
pub fn accept_recommended(settings: &Settings) -> bool {
    settings.accept_recommended.unwrap_or(false)
}

/// How long zircd holds a yes to a request that can't be undone, as `settings` sets it: a
/// minute unless set.
pub fn request_hold(settings: &Settings) -> std::time::Duration {
    std::time::Duration::from_secs(
        settings
            .request_hold
            .unwrap_or(zirc_proto::request::HOLD_SECONDS),
    )
}

fn connection(settings: &Settings, org: Organisation, device_key: &Path) -> Config {
    let nick = org.nick.unwrap_or_else(default_nick);
    let mut config = settings::connection(&org.server, nick);
    // A machine says whose it is in its real name; one without an owner gives its nick.
    if let Some(owner) = org.owner {
        config.realname = owner;
    }
    // Its own messages come back with the server's id, so a reaction to one can be told
    // apart as its session's.
    config.caps.push("echo-message".to_owned());
    // A host set while it's on, such as its own once the server says whose it is.
    config.caps.push("chghost".to_owned());
    if let Some(seconds) = settings.keepalive {
        config.keepalive = std::time::Duration::from_secs(seconds);
        config.answer_within = config.keepalive / 2;
    }
    // With the machine's own key, where the server has it, and the passphrase for when the
    // key makes no token.
    if let Some(account) = org.account.as_ref().filter(|a| a.device_key) {
        let key = std::sync::Arc::new(zirc_core::device_key::Software::in_file(
            device_key.to_owned(),
        ));
        config.bearer = Some(zirc_core::device_key::bearer(
            key,
            account.name.clone(),
            org.server.host.clone(),
        ));
    }
    config.sasl = org.account.map(|account| Credentials {
        account: account.name,
        password: account.password,
    });
    config
}

/// `<user>-<host>`, so each machine has a nick of its own.
fn default_nick() -> String {
    let user = std::env::var("USER").unwrap_or_else(|_| "zirc".to_owned());
    let host = zirc_core::machine::host_name();
    if host.is_empty() {
        names::nick_from(&user)
    } else {
        names::nick_from(&format!("{user}-{host}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str, org: &str) -> Result<Config> {
        let settings = Settings::from_toml(text)?;
        let org = find(&settings, org, false).context("not signed in to it")?;
        Ok(connection(
            &settings,
            org,
            Path::new("/nonexistent/device-key"),
        ))
    }

    #[test]
    fn reads_a_full_config() {
        let config = parse(
            r#"
            nick = "me"
            [server]
            host = "irc.zirc.dev"
            password = "gate"
            [account]
            name = "mattias"
            password = "pw"
            "#,
            // zirc.dev's public server, by the name zirc gives it.
            "public",
        )
        .unwrap();
        assert_eq!(
            (config.host.as_str(), config.port, config.tls),
            ("irc.zirc.dev", 6697, true)
        );
        assert_eq!(config.nick, "me");
        assert_eq!(config.server_password.as_deref(), Some("gate"));
        assert_eq!(config.sasl.unwrap().account, "mattias");
    }

    #[test]
    fn a_machine_with_its_key_on_the_server_signs_in_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let text = r#"
            [server]
            host = "zirc.irc.zirc.dev"
            [account]
            name = "mattias-mbp"
            password = "pw"
            device_key = true
            "#;
        let settings = Settings::from_toml(text).unwrap();
        let org = find(&settings, "zirc", true).unwrap();
        let config = connection(&settings, org, &dir.path().join("device-key"));
        let token = (config.bearer.unwrap())().unwrap();
        let claims = zirc_proto::device_key::parse(&token).unwrap().claims;
        assert_eq!(
            (claims.sub.as_str(), claims.aud.as_str()),
            ("mattias-mbp", "zirc.irc.zirc.dev")
        );
        // The passphrase stays, for a server that refuses the key.
        assert_eq!(config.sasl.unwrap().password, "pw");

        let without = parse(&text.replace("device_key = true", ""), "zirc").unwrap();
        assert!(without.bearer.is_none());
    }

    #[test]
    fn reads_each_organisations_connection() {
        let text = r#"
            nick = "m-zirc"
            owner = "mattias"
            [server]
            host = "zirc.irc.zirc.dev"
            [[org]]
            nick = "m-globex"
            owner = "matt"
            [org.server]
            host = "globex.irc.zirc.dev"
            [org.account]
            name = "m-globex"
            password = "pw"
            "#;
        let zirc = parse(text, "zirc").unwrap();
        assert_eq!(
            (
                zirc.host.as_str(),
                zirc.nick.as_str(),
                zirc.realname.as_str()
            ),
            ("zirc.irc.zirc.dev", "m-zirc", "mattias")
        );
        assert!(zirc.sasl.is_none());
        let globex = parse(text, "globex").unwrap();
        assert_eq!(
            (
                globex.host.as_str(),
                globex.nick.as_str(),
                globex.realname.as_str()
            ),
            ("globex.irc.zirc.dev", "m-globex", "matt")
        );
        assert_eq!(globex.sasl.unwrap().account, "m-globex");
        assert!(parse(text, "initech").is_err());
    }

    #[test]
    fn finds_the_first_organisation_after_its_server_moved() {
        let moved = Settings::from_toml("[server]\nhost = \"new.irc.zirc.dev\"\n").unwrap();
        assert_eq!(find(&moved, "old", true).unwrap().name(), "new");
        assert!(find(&moved, "old", false).is_none());
    }

    #[test]
    fn refuses_an_organisation_it_cant_keep_a_directory_for() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[server]\nhost = \"zirc.irc.zirc.dev\"\n[[org]]\nname = \"../x\"\n[org.server]\nhost = \"x\"\n",
        )
        .unwrap();
        let refused = organisations(&read(&path).unwrap(), &path)
            .unwrap_err()
            .to_string();
        assert!(refused.contains("\"../x\""), "{refused}");
    }

    #[test]
    fn a_running_zircd_reads_the_file_again_only_when_it_changes() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("config.toml");
        let file = File::at(&path);
        let every = |file: &File| file.ask(|settings| settings.announce_every);
        // Not there yet, as before `zirc login`: nothing to say, and no failure.
        assert_eq!(every(&file), None);
        let write = |seconds: u32| {
            let text = format!("announce_every = {seconds}\n\n[server]\nhost = \"irc.example\"\n");
            std::fs::write(&path, text).unwrap();
        };
        write(60);
        assert_eq!(every(&file), Some(60));
        // An edit counts without a restart: a longer file, whatever its time says.
        write(1200);
        assert_eq!(every(&file), Some(1200));
        // Unreadable, as halfway through a write: nothing, until it reads again.
        std::fs::write(&path, "announce_every = [").unwrap();
        assert_eq!(every(&file), None);
        write(90);
        assert_eq!(every(&file), Some(90));
    }
}
