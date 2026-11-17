//! Setting a machine up: `zirc login` and `zirc init`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, bail};
use clap::Args;
use zirc_core::device_key::{DeviceKey, Software};
use zirc_core::ipc::{self, Request, Response};
use zirc_core::login::{self, Client};
use zirc_core::paths;
use zirc_core::settings::{Account, Organisation, Server, Settings};
use zirc_proto::PROGRAM;
use zirc_proto::login::{Credentials, LoginRequest};

use crate::client::{shown, tail};
use crate::daemon::{stop, zircd_binary};
use crate::repo::RepoCommand;
use crate::skill::install_skill;
use crate::{guest, service};

#[derive(Debug, Args)]
pub(crate) struct Login {
    /// A public channel to connect to as its guest, such as acme/web, or its chat's address.
    #[arg(value_name = "CHANNEL")]
    pub(crate) target: Option<String>,
    /// Zirc's web app, where you log in.
    #[arg(long, env = "ZIRC_WEB", value_name = "URL", default_value = login::WEB)]
    pub(crate) web: String,
    /// Don't open the browser: open the URL zirc shows yourself.
    #[arg(long)]
    pub(crate) no_browser: bool,
    /// Only write config.toml and install the skill: leave zircd as it is.
    #[arg(long)]
    pub(crate) no_daemon: bool,
    /// Replace this machine's setup, if it has one, with the login's.
    #[arg(long)]
    pub(crate) force: bool,
    /// Add this organisation to those the machine is signed in to, or log in to it again.
    /// Hidden, as every --org is: someone in one organisation sees no change.
    #[arg(long, value_name = "NAME", hide = true)]
    pub(crate) org: Option<String>,
    /// The zircd to run. Next to this zirc, or on the PATH, unless set.
    #[arg(long, env = "ZIRC_ZIRCD", hide = true)]
    pub(crate) zircd: Option<PathBuf>,
    /// Claude Code's skills directory. ~/.claude/skills unless set.
    #[arg(long, hide = true)]
    pub(crate) skills: Option<PathBuf>,
}

impl Login {
    /// The login `zirc repo add` starts on a machine that isn't set up yet: to zirc.dev, or
    /// `ZIRC_WEB`, as `zirc login` with no flags would be.
    pub(crate) fn first() -> Self {
        Self {
            target: None,
            web: std::env::var("ZIRC_WEB").unwrap_or_else(|_| login::WEB.to_owned()),
            no_browser: false,
            no_daemon: false,
            force: false,
            org: None,
            zircd: std::env::var_os("ZIRC_ZIRCD").map(PathBuf::from),
            skills: None,
        }
    }
}

/// Whether this machine has no setup yet: nothing logged in, and no server of its own.
pub(crate) fn unset(home: &Path) -> Result<bool> {
    Ok(earlier_setup(&paths::config(home), false)?.is_none())
}

#[derive(Debug, Args)]
pub(crate) struct Init {
    /// The IRC server's host name, such as irc.zirc.dev.
    #[arg(long)]
    server: String,
    /// Its port. 6697 unless set.
    #[arg(long)]
    port: Option<u16>,
    /// Connect without TLS, for a local server only.
    #[arg(long)]
    no_tls: bool,
    /// The nick to use. <user>-<host> unless set.
    #[arg(long)]
    nick: Option<String>,
    /// Whose machine this is: their nick, so their apps can message its agents.
    #[arg(long, value_name = "NICK")]
    owner: Option<String>,
    /// Replace an existing config.toml.
    #[arg(long)]
    force: bool,
}

/// The setup already in `path`, if any. One that can't be read may be all that's left of
/// a setup, so it's replaced only when `force` says to.
pub(crate) fn earlier_setup(path: &Path, force: bool) -> Result<Option<Settings>> {
    match std::fs::read_to_string(path) {
        Ok(text) => match Settings::from_toml(&text) {
            Ok(settings) => Ok(Some(settings)),
            Err(_) if force => Ok(None),
            Err(err) => bail!(
                "{} isn't readable ({err}); fix it, or replace it with `zirc login --force`",
                path.display()
            ),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound || force => Ok(None),
        Err(err) => bail!("couldn't read {}: {err}", path.display()),
    }
}

pub(crate) async fn login(home: &Path, args: Login, out: &mut (impl Write + Send)) -> Result<()> {
    logged_in_then(home, args, true, out).await
}

/// `zirc repo`, whose `add` is the first step and needs nothing before it: on a machine that
/// isn't set up, it signs in first, as `zirc login` does, then carries on adding the repository.
pub(crate) async fn repo_first(
    home: &Path,
    socket: &Path,
    command: RepoCommand,
    out: &mut (impl Write + Send),
) -> Result<()> {
    if matches!(command, RepoCommand::Add { .. }) && unset(home)? {
        writeln!(
            out,
            "This machine isn't signed in to Zirc yet: signing in first."
        )?;
        writeln!(out)?;
        logged_in_then(home, Login::first(), false, out).await?;
        writeln!(out)?;
    }
    crate::repo::repo(socket, command, out).await
}

/// The login itself; `next` says whether to end on what to do next, `zirc repo add`.
async fn logged_in_then(
    home: &Path,
    args: Login,
    next: bool,
    out: &mut (impl Write + Send),
) -> Result<()> {
    // A machine that's set up keeps its setup, and its gate password, unless the login is
    // for the same server (an upgrade, such as to an account of its own) or it's asked to.
    let path = paths::config(home);
    let earlier = earlier_setup(&path, args.force)?;
    if let (Some(earlier), false) = (&earlier, args.force) {
        writeln!(
            out,
            "This machine is set up for {}. Logging in to it again updates that; another \
             server needs `zirc login --force`.",
            earlier.server.host
        )?;
        writeln!(out)?;
    }

    let credentials = confirmed_in_browser(home, &args, out).await?;
    if let Some(account_id) = &credentials.account_id {
        zirc_core::telemetry::link(home, account_id);
    }
    // A web app that signs in to several organisations, or --org, adds to the machine's
    // setup; otherwise a login for another server than the machine's is refused, as before.
    if let (Some(earlier), false) = (&earlier, args.force)
        && credentials.orgs.is_empty()
        && args.org.is_none()
        && earlier.guest.is_none()
        && earlier.server.host != credentials.server.host
    {
        bail!(
            "that login is for {}, but this machine is set up for {}: nothing changed. Run \
             `zirc login --force` to switch it",
            credentials.server.host,
            earlier.server.host
        );
    }

    let logged_in = logged_in(&credentials, &args.web)?;
    let settings = login_settings(&logged_in, earlier.as_ref(), args.force);
    settings.save(&path)?;
    link_organisations(home, &credentials, &args.web);
    writeln!(out)?;
    for org in &logged_in {
        let how = org
            .account
            .as_ref()
            .map_or_else(String::new, |account| format!(" as {}", account.name));
        writeln!(
            out,
            "Logged in: this machine connects to {}{how}, as set in {}",
            org.server.host,
            shown(&path)
        )?;
    }
    let skill = install_skill(args.skills)?;
    writeln!(
        out,
        "Installed the {PROGRAM} skill for Claude Code: {}",
        shown(&skill)
    )?;
    if args.no_daemon {
        writeln!(
            out,
            "zircd isn't started: run it at login with `zirc daemon install`, or until you stop \
             it with `zirc daemon start`"
        )?;
        return Ok(());
    }
    writeln!(
        out,
        "Starting zircd at login, and waiting for it to connect…"
    )?;
    // A zircd started by hand stops first, so the one at login is the only one.
    let socket = paths::socket(home);
    let _ = stop(&socket, &mut std::io::sink()).await;
    service::install(home, &zircd_binary(args.zircd)?, &mut std::io::sink())?;
    connected_to(home, None, next, out).await
}

/// Starts a login for this machine at `args.web`, says where to confirm it, and waits until
/// the person has: what to connect with.
async fn confirmed_in_browser(
    home: &Path,
    args: &Login,
    out: &mut (impl Write + Send),
) -> Result<Credentials> {
    let client = Client::new(&args.web)?;
    let request = LoginRequest {
        guest: None,
        client: format!("{PROGRAM} {}", env!("CARGO_PKG_VERSION")),
        host: zirc_core::machine::host_name(),
        // A machine's login, the default, which zirc.dev also assumes from older CLIs.
        kind: None,
        orgs: Some(true),
        org: args.org.clone(),
        machine_id: zirc_core::machine::id(home),
        // The machine's own key, which zircd signs in with where the server takes it.
        device_key: DeviceKey::public_key(&Software::machine(home)),
        connected: None,
    };
    let started = client.start(&request).await?;
    writeln!(
        out,
        "Open this link to sign in and connect this machine to your team:"
    )?;
    writeln!(out)?;
    writeln!(out, "  {}", started.url)?;
    writeln!(out)?;
    writeln!(
        out,
        "It shows the code {}; check that it's this one.",
        started.code
    )?;
    if !args.no_browser {
        zirc_core::login::open_browser(&started.url);
    }
    writeln!(
        out,
        "Waiting for you there. The link works for {}; Ctrl-C stops waiting.",
        zirc_core::login::link_lifetime(&started)
    )?;
    out.flush()?;
    Ok(client.wait(&started).await?)
}

/// The organisations a login's `credentials` from `web` sign this machine in to, the first
/// first. Each account's name is its nick, unless the web app has one for this machine.
/// Remembers each organisation's opaque id a login at `web` gave, which analytics groups its
/// events by.
pub(crate) fn link_organisations(home: &Path, credentials: &Credentials, web: &str) {
    for org in zirc_core::login::each_org(credentials, web).unwrap_or_default() {
        if let Some(id) = &org.credentials.organisation_id {
            let name = org
                .name
                .unwrap_or_else(|| zirc_core::settings::org_name(&org.credentials.server));
            zirc_core::telemetry::link_organisation(home, &name, id);
        }
    }
}

pub(crate) fn logged_in(credentials: &Credentials, web: &str) -> Result<Vec<Organisation>> {
    let nick = |nick: &Option<String>, account: &Option<Account>| {
        nick.clone()
            .or_else(|| account.as_ref().map(|account| account.name.clone()))
    };
    let orgs = zirc_core::login::each_org(credentials, web)
        .map_err(|err| anyhow::anyhow!("{err}: nothing changed"))?;
    Ok(orgs
        .into_iter()
        .map(|org| Organisation {
            name: org.name,
            nick: nick(&org.credentials.nick, &org.credentials.account),
            server: org.credentials.server,
            account: org.credentials.account,
            owner: org.credentials.owner,
            web: Some(web.to_owned()),
            guest: None,
        })
        .collect())
}

/// The settings logging in to `orgs` gives: the `earlier` ones with those organisations set,
/// the others staying signed in, or with `replace`, those organisations alone. What's the
/// machine's own stays, such as a nick chosen earlier the web app has none for.
pub(crate) fn login_settings(
    orgs: &[Organisation],
    earlier: Option<&Settings>,
    replace: bool,
) -> Settings {
    let first = &orgs[0];
    let mut settings = earlier.cloned().unwrap_or_else(|| Settings {
        nick: None,
        server: first.server.clone(),
        account: None,
        owner: None,
        web: None,
        keepalive: None,
        auto_update: None,
        announce_every: None,
        request_hold: None,
        accept_recommended: None,
        telemetry: None,
        posthog_key: None,
        posthog_host: None,
        leases: std::collections::BTreeMap::new(),
        wakes: None,
        guest: None,
        orgs: Vec::new(),
    });
    let replace = replace || earlier.is_none();
    if replace {
        settings.orgs.clear();
    }
    // A machine that was only a guest's terminal logs in to its own organisation first: the
    // terminal goes after it.
    let guest_first = !replace
        && settings.organisations()[0].guest.is_some()
        && first.guest.is_none()
        && settings.organisation(&first.name()).is_none();
    for (at, org) in orgs.iter().enumerate() {
        if guest_first && at == 0 {
            settings.put_first(org.clone());
            continue;
        }
        // Replacing, the first takes the place of the machine's first.
        let name = if replace && at == 0 {
            settings.organisations()[0].name()
        } else {
            org.name()
        };
        let mut org = org.clone();
        if let Some(before) = settings.organisation(&name) {
            org.nick = org.nick.or(before.nick);
        }
        settings.set_organisation(&name, org);
    }
    settings
}

/// How long `zirc login` waits for zircd to connect.
pub(crate) const CONNECT_WAIT: Duration = Duration::from_secs(20);

/// Waits for the zircd just started to connect, and says how it went: connected and what
/// to do next (unless `next` is false), or what its log says went wrong. With `guest`, its
/// connection as that guest's terminal.
pub(crate) async fn connected_to(
    home: &Path,
    guest: Option<&str>,
    next: bool,
    out: &mut impl Write,
) -> Result<()> {
    let socket = paths::socket(home);
    let deadline = tokio::time::Instant::now() + CONNECT_WAIT;
    loop {
        let asked = tokio::time::timeout_at(deadline, ipc::request(&socket, &Request::Status));
        let asked = asked.await;
        if let (Some(name), Ok(Ok(status))) = (guest, &asked)
            && let Some(lines) = guest::connected_lines(status, name)
        {
            for line in lines {
                writeln!(out, "{line}")?;
            }
            return Ok(());
        }
        if let (None, Ok(Ok(Response::Status(status)))) = (guest, asked)
            && status.connected
        {
            let nick = status.nick.unwrap_or_default();
            writeln!(
                out,
                "zircd is running at login, connected to {} as {nick}.",
                status.server
            )?;
            if next {
                writeln!(out)?;
                writeln!(out, "Next, in each repository you work in: zirc repo add")?;
            }
            return Ok(());
        }
        if tokio::time::Instant::now() > deadline {
            let log = paths::log(home);
            bail!(
                "zircd runs at login, but hasn't connected within {CONNECT_WAIT:?}. The end of {}:\n{}",
                shown(&log),
                tail(&std::fs::read_to_string(&log).unwrap_or_default())
            );
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

pub(crate) fn init(home: &Path, init: Init, out: &mut impl Write) -> Result<()> {
    let path = paths::config(home);
    if path.exists() && !init.force {
        bail!(
            "{} already exists: pass --force to replace it",
            shown(&path)
        );
    }
    let settings = Settings {
        nick: init.nick,
        server: Server {
            host: init.server,
            port: init.port,
            tls: init.no_tls.then_some(false),
            password: None,
        },
        account: None,
        owner: init.owner,
        web: None,
        keepalive: None,
        auto_update: None,
        announce_every: None,
        request_hold: None,
        accept_recommended: None,
        telemetry: None,
        posthog_key: None,
        posthog_host: None,
        leases: std::collections::BTreeMap::new(),
        wakes: None,
        guest: None,
        orgs: Vec::new(),
    };
    settings.save(&path)?;
    writeln!(out, "wrote {}", shown(&path))?;
    writeln!(
        out,
        "add a server password or an [account] there if your server needs them"
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    use crate::{Cli, Command, run};

    #[tokio::test]
    async fn says_to_log_in_on_a_machine_that_isnt_set_up() {
        let home = tempfile::tempdir().unwrap();
        let run_in = |args: &[&str]| {
            let cli = Cli::try_parse_from(args).unwrap();
            let cli = Cli {
                home: Some(home.path().to_owned()),
                ..cli
            };
            async move { run(cli, &mut Vec::new()).await }
        };
        for args in [
            &["zirc", "daemon", "start", "--zircd", "/nonexistent"][..],
            &["zirc", "daemon", "install", "--zircd", "/nonexistent"],
            &["zirc", "status"],
            &["zirc", "who"],
        ] {
            let err = run_in(args).await.unwrap_err().to_string();
            assert!(err.contains("run `zirc login`"), "{args:?}: {err}");
            assert!(
                err.contains("`zirc init --server <host>`"),
                "{args:?}: {err}"
            );
        }
        // Set up, but zircd stopped: start it.
        std::fs::write(paths::config(home.path()), "[server]\nhost = \"x\"\n").unwrap();
        let err = run_in(&["zirc", "status"]).await.unwrap_err().to_string();
        assert_eq!(
            err,
            "zircd isn't running: start it with `zirc daemon start`"
        );
    }

    /// zirc.dev's stand-in, answering with `answers` in turn, and the first line and body
    /// of each request it got.
    fn fake_web(answers: Vec<(u16, String)>) -> (String, impl FnOnce() -> Vec<String>) {
        let (web, served) = zirc_test_ergo::web::serve(answers);
        let requests = move || {
            served
                .join()
                .unwrap()
                .iter()
                .map(|request| {
                    let first = request.lines().next().unwrap_or_default();
                    let body = request.split("\r\n\r\n").nth(1).unwrap_or_default();
                    format!("{first} {body}").trim_end().to_owned()
                })
                .collect()
        };
        (web, requests)
    }

    #[tokio::test]
    async fn logs_in_and_writes_the_teams_server() {
        let started = r#"{"id":"l1","code":"WDJB-MJHT","url":"https://zirc.dev/login/WDJB-MJHT","interval":1,"expires_in":30}"#;
        let done = r#"{"status":"done","server":{"host":"acme.irc.zirc.dev","password":"gate"},"account":{"name":"mattias","password":"pw"}}"#;
        let (web, served) = fake_web(vec![
            (201, started.into()),
            (200, r#"{"status":"pending"}"#.into()),
            (200, done.into()),
        ]);
        let home = tempfile::tempdir().unwrap();
        let skills = tempfile::tempdir().unwrap();
        let earlier = "nick = \"me\"\n[server]\nhost = \"old.example\"\n";
        std::fs::write(paths::config(home.path()), earlier).unwrap();

        let cli = Cli::try_parse_from([
            "zirc",
            "login",
            "--web",
            &web,
            "--no-browser",
            "--no-daemon",
            "--force",
            "--skills",
            skills.path().to_str().unwrap(),
        ])
        .unwrap();
        let cli = Cli {
            home: Some(home.path().to_owned()),
            ..cli
        };
        let mut out = Vec::new();
        run(cli, &mut out).await.unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.contains("\n  https://zirc.dev/login/WDJB-MJHT\n"),
            "{out}"
        );
        assert!(out.contains("It shows the code WDJB-MJHT;"), "{out}");
        assert!(out.contains("The link works for 1 minute;"), "{out}");
        assert!(
            out.contains("this machine connects to acme.irc.zirc.dev"),
            "{out}"
        );

        // The skill comes with it, for the coding agents here.
        let skill = std::fs::read_to_string(skills.path().join("zirc/SKILL.md")).unwrap();
        assert!(skill.contains("zirc watch"), "{skill}");
        assert!(out.contains("Installed the zirc skill"), "{out}");

        let requests = served();
        assert!(
            requests[0].starts_with("POST /api/cli/login "),
            "{requests:?}"
        );
        assert!(requests[0].contains(r#""client":"zirc "#), "{requests:?}");
        assert!(
            requests[1].starts_with("GET /api/cli/login/l1 "),
            "{requests:?}"
        );

        let path = paths::config(home.path());
        let settings = Settings::from_toml(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // The account's name is its nick, whatever nick the machine had.
        assert_eq!(settings.nick.as_deref(), Some("mattias"));
        assert_eq!(settings.server.host, "acme.irc.zirc.dev");
        assert_eq!(settings.server.password.as_deref(), Some("gate"));
        assert_eq!(settings.account.unwrap().name, "mattias");
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(&path).unwrap().permissions(),
        );
        assert_eq!(mode & 0o777, 0o600, "config.toml holds passwords");
    }

    /// `zirc login` on a machine with `earlier` config, for a login that comes back for
    /// `host`: what it says, or why it refused, and the config afterwards.
    async fn login_again(earlier: &str, host: &str) -> (Result<String, String>, String) {
        let done = format!(
            r#"{{"status":"done","server":{{"host":"{host}","password":"gate"}},"account":{{"name":"mattias-mbp","password":"pass"}},"nick":"mattias-mbp","owner":"mattias"}}"#
        );
        login_with(earlier, done).await
    }

    /// `zirc login` on a machine with `earlier` config, for a login that ends with `done`.
    async fn login_with(earlier: &str, done: String) -> (Result<String, String>, String) {
        let started = r#"{"id":"l1","code":"C","url":"u","interval":1,"expires_in":30}"#;
        let (web, _served) = fake_web(vec![(201, started.into()), (200, done)]);
        let home = tempfile::tempdir().unwrap();
        let skills = tempfile::tempdir().unwrap();
        std::fs::write(paths::config(home.path()), earlier).unwrap();
        let cli = Cli::try_parse_from([
            "zirc",
            "login",
            "--web",
            &web,
            "--no-browser",
            "--no-daemon",
            "--skills",
            skills.path().to_str().unwrap(),
        ])
        .unwrap();
        let cli = Cli {
            home: Some(home.path().to_owned()),
            ..cli
        };
        let mut out = Vec::new();
        let result = run(cli, &mut out).await;
        let config = std::fs::read_to_string(paths::config(home.path())).unwrap();
        (
            result
                .map(|()| String::from_utf8(out).unwrap())
                .map_err(|err| err.to_string()),
            config,
        )
    }

    #[tokio::test]
    async fn names_the_machine_as_the_web_app_says_then_its_account_then_as_before() {
        let earlier = "nick = \"old-nick\"\n[server]\nhost = \"acme.irc.zirc.dev\"\n";
        let nick_of = |config: &str| Settings::from_toml(config).unwrap().nick;
        let server = r#""server":{"host":"acme.irc.zirc.dev","password":"gate"}"#;
        let account = r#""account":{"name":"mattias-mbp","password":"pass"}"#;
        for (done, nick) in [
            (
                format!(r#"{{"status":"done",{server},{account},"nick":"chosen"}}"#),
                "chosen",
            ),
            (
                format!(r#"{{"status":"done",{server},{account}}}"#),
                "mattias-mbp",
            ),
            (format!(r#"{{"status":"done",{server}}}"#), "old-nick"),
        ] {
            let (said, config) = login_with(earlier, done).await;
            said.unwrap();
            assert_eq!(nick_of(&config).as_deref(), Some(nick));
        }
    }

    #[tokio::test]
    async fn upgrades_a_machine_logging_in_to_its_own_server_again() {
        let gate_only = "nick = \"mattias-matt\"\n[server]\nhost = \"zircapp.irc.zirc.dev\"\npassword = \"gate\"\n";
        let (said, config) = login_again(gate_only, "zircapp.irc.zirc.dev").await;
        let said = said.unwrap();
        assert!(said.contains("set up for zircapp.irc.zirc.dev"), "{said}");
        assert!(said.contains("as mattias-mbp"), "{said}");
        let settings = Settings::from_toml(&config).unwrap();
        assert_eq!(settings.account.unwrap().name, "mattias-mbp");
        assert_eq!(settings.nick.as_deref(), Some("mattias-mbp"));
        assert_eq!(settings.owner.as_deref(), Some("mattias"));
        assert!(settings.web.unwrap().starts_with("http://127.0.0.1:"));
    }

    #[tokio::test]
    async fn adds_the_organisations_a_login_chose_and_keeps_the_others() {
        let earlier = "nick = \"mattias-mbp\"\n[server]\nhost = \"acme.irc.zirc.dev\"\n\n\
                       [[org]]\nname = \"initech\"\n[org.server]\nhost = \"initech.irc.zirc.dev\"\n";
        let globex = r#"{"name":"globex","server":{"host":"globex.irc.zirc.dev"},"account":{"name":"matt-mbp","password":"pw"},"owner":"matt"}"#;
        let acme = r#"{"name":"acme","server":{"host":"acme.irc.zirc.dev"},"owner":"mattias"}"#;
        let done = format!(
            r#"{{"status":"done","server":{{"host":"acme.irc.zirc.dev"}},"owner":"mattias","orgs":[{acme},{globex}]}}"#
        );
        let (said, config) = login_with(earlier, done).await;
        let said = said.unwrap();
        assert!(said.contains("connects to acme.irc.zirc.dev"), "{said}");
        assert!(
            said.contains("connects to globex.irc.zirc.dev as matt-mbp"),
            "{said}"
        );
        let settings = Settings::from_toml(&config).unwrap();
        let names: Vec<String> = settings
            .organisations()
            .iter()
            .map(Organisation::name)
            .collect();
        assert_eq!(names, ["acme", "initech", "globex"]);
        // Each organisation keeps its own handle and nick; acme's nick chosen earlier stays.
        assert_eq!(settings.nick.as_deref(), Some("mattias-mbp"));
        let globex = settings.organisation("globex").unwrap();
        assert_eq!(
            (globex.nick.as_deref(), globex.owner.as_deref()),
            (Some("matt-mbp"), Some("matt"))
        );
    }

    #[tokio::test]
    async fn leaves_a_config_it_cant_read_alone_unless_forced() {
        let broken = "[server\nhost = \"zircapp.irc.zirc.dev\"\n";
        let (refused, config) = login_again(broken, "zircapp.irc.zirc.dev").await;
        let refused = refused.unwrap_err();
        assert!(refused.contains("isn't readable"), "{refused}");
        assert!(refused.contains("zirc login --force"), "{refused}");
        assert_eq!(config, broken);
    }

    #[tokio::test]
    async fn keeps_a_machines_setup_unless_asked_to_replace_it() {
        let earlier = "[server]\nhost = \"zircapp.irc.zirc.dev\"\npassword = \"gate\"\n";
        let (refused, config) = login_again(earlier, "acme.irc.zirc.dev").await;
        let refused = refused.unwrap_err();
        assert!(
            refused.contains("for acme.irc.zirc.dev, but this machine is set up for zircapp"),
            "{refused}"
        );
        assert!(refused.contains("`zirc login --force`"), "{refused}");
        assert_eq!(config, earlier);
    }

    #[tokio::test]
    async fn says_when_the_web_app_takes_no_logins_or_the_login_fails() {
        let started = r#"{"id":"l1","code":"C","url":"u","interval":1,"expires_in":30}"#;
        for (answers, said) in [
            (
                vec![(501, r#"{"error":"not implemented"}"#.to_owned())],
                "doesn't take logins yet",
            ),
            (
                vec![
                    (201, started.to_owned()),
                    (200, r#"{"status":"denied"}"#.to_owned()),
                ],
                "turned down",
            ),
            (
                vec![(201, started.to_owned()), (404, "{}".to_owned())],
                "expired",
            ),
            // Away for a moment, then back with an answer: the login goes on.
            (
                vec![
                    (201, started.to_owned()),
                    (503, "{}".to_owned()),
                    (200, r#"{"status":"denied"}"#.to_owned()),
                ],
                "turned down",
            ),
            (
                vec![(429, r#"{"error":"slow down"}"#.to_owned())],
                "answered 429 Too Many Requests: slow down",
            ),
        ] {
            let (web, _served) = fake_web(answers);
            let home = tempfile::tempdir().unwrap();
            let cli =
                Cli::try_parse_from(["zirc", "login", "--web", &web, "--no-browser"]).unwrap();
            let cli = Cli {
                home: Some(home.path().to_owned()),
                ..cli
            };
            let err = run(cli, &mut Vec::new()).await.unwrap_err();
            assert!(err.to_string().contains(said), "{said}: {err}");
            assert!(!paths::config(home.path()).exists());
        }
    }

    #[test]
    fn writes_a_config_zircd_can_read() {
        let home = tempfile::tempdir().unwrap();
        let cli = Cli::try_parse_from(["zirc", "init", "--server", "irc.zirc.dev", "--nick", "me"])
            .unwrap();
        let Command::Init(args) = cli.command else {
            panic!()
        };
        init(home.path(), args, &mut Vec::new()).unwrap();
        let text = std::fs::read_to_string(paths::config(home.path())).unwrap();
        let settings = Settings::from_toml(&text).unwrap();
        assert_eq!(settings.server.host, "irc.zirc.dev");
        assert_eq!(settings.nick.as_deref(), Some("me"));
        // And it doesn't overwrite without being asked.
        let again = Cli::try_parse_from(["zirc", "init", "--server", "other"]).unwrap();
        let Command::Init(args) = again.command else {
            panic!()
        };
        assert!(init(home.path(), args, &mut Vec::new()).is_err());
    }
}
