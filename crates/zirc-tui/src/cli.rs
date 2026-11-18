//! The command line, and the connection it describes: as the person, signed in through
//! zirc.dev, or as the flags and zirc's settings say.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};
use zirc_core::login;
use zirc_core::secrets::Secrets;
use zirc_core::settings::{Server, Settings, connection};
use zirc_irc::{Config, Credentials};
use zirc_theme::Theme;

use crate::{App, signin};

/// Capabilities asked for on top of zirc-irc's, as every app asks: see
/// [`zirc_core::connection::APP_CAPS`].
pub const EXTRA_CAPS: &[&str] = zirc_core::connection::APP_CAPS;

/// A terminal IRC client for your team's Zirc server, signed in as yourself.
#[derive(Debug, Parser)]
#[command(name = "zirc tui")]
pub struct Cli {
    /// What to do instead of chatting.
    #[command(subcommand)]
    pub command: Option<Command>,
    /// zirc.dev, where you sign in.
    #[arg(long, env = "ZIRC_WEB", value_name = "URL", default_value = login::WEB, global = true)]
    pub web: String,
    /// Print the sign-in link without opening the browser.
    #[arg(long, global = true)]
    pub no_browser: bool,
    /// zirc's settings, to connect as they say instead of as you signed in:
    /// `~/.zirc/config.toml` when there's no sign-in and zirc.dev doesn't take them.
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,
    /// The server, instead of the one in the settings, as `host` or `host:port`.
    #[arg(long, value_name = "HOST[:PORT]")]
    pub server: Option<String>,
    /// Connect without TLS, as to a local server.
    #[arg(long)]
    pub no_tls: bool,
    /// The nick to use: your sign-in's, your account's, your handle from the settings, or
    /// `$USER`.
    #[arg(long)]
    pub nick: Option<String>,
    /// Your account, to log in to with SASL instead of the sign-in's. The settings' own
    /// account is the machine's, so it isn't used.
    #[arg(long, requires = "password")]
    pub account: Option<String>,
    /// The account's password.
    #[arg(long, env = "ZIRC_TUI_PASSWORD", hide_env_values = true)]
    pub password: Option<String>,
    /// A channel to join besides those the server lists; repeatable.
    #[arg(long = "join", value_name = "CHANNEL")]
    pub joins: Vec<String>,
    /// Leave the mouse to the terminal, for selecting text, rather than clicking on
    /// conversations and scrolling with the wheel.
    #[arg(long)]
    pub no_mouse: bool,
    /// zirc's colours: dark, light, or whichever matches the terminal's background.
    #[arg(long, env = "ZIRC_TUI_THEME", value_enum, default_value_t = ThemeChoice::Auto)]
    pub theme: ThemeChoice,
}

/// Which of zirc's themes to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ThemeChoice {
    /// Dark on a dark terminal, light on a light one; dark when the terminal won't say.
    Auto,
    /// Dark.
    Dark,
    /// Light.
    Light,
}

impl ThemeChoice {
    /// The theme, asking the terminal its background colour for [`ThemeChoice::Auto`]. Do
    /// it before the screen is taken over: the terminal answers on standard input.
    #[must_use]
    pub fn theme(self) -> &'static Theme {
        match self {
            Self::Dark => Theme::dark(),
            Self::Light => Theme::light(),
            Self::Auto => {
                match terminal_colorsaurus::theme_mode(terminal_colorsaurus::QueryOptions::default())
                {
                    Ok(terminal_colorsaurus::ThemeMode::Light) => Theme::light(),
                    Ok(terminal_colorsaurus::ThemeMode::Dark) | Err(_) => Theme::dark(),
                }
            }
        }
    }
}

/// What to do instead of chatting.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Sign in as yourself through zirc.dev.
    ///
    /// Your account's passphrase is kept in the system's secret store.
    Login,
    /// Forget the sign-in and its passphrase.
    Logout {
        /// Only this organisation's, on a client signed in to several. Hidden, as every
        /// --org is: someone in one organisation sees no change.
        #[arg(long, value_name = "NAME", hide = true)]
        org: Option<String>,
    },
}

impl Cli {
    /// Does what the command line says, with zirc's directory `home`: signs in or out, or
    /// chats in the terminal until it's quit. What it says goes to `out`.
    ///
    /// # Errors
    ///
    /// When signing in or out fails, or as [`crate::run`].
    pub async fn run(self, home: &Path, out: &mut (impl Write + Send)) -> Result<()> {
        let secrets = zirc_core::secrets::system(home);
        match self.command {
            Some(Command::Login) => {
                signin::sign_in(&self.web, !self.no_browser, home, secrets.as_ref(), out).await?;
                writeln!(out, "Run zirc tui to chat.")?;
                Ok(())
            }
            Some(Command::Logout { org }) => {
                let secrets: std::sync::Arc<dyn zirc_core::secrets::Secrets> = secrets.into();
                let key = zirc_core::device_key::of(&secrets);
                signin::sign_out(home, secrets.as_ref(), &key, org.as_deref(), out).await
            }
            None => {
                let prefs = crate::prefs::path(home);
                let mut orgs = Vec::new();
                for (at, (org, config)) in self
                    .connections(home, secrets.as_ref(), out)
                    .await?
                    .into_iter()
                    .enumerate()
                {
                    // The channels named on the command line are the first organisation's.
                    let joins = if at == 0 {
                        self.joins.clone()
                    } else {
                        Vec::new()
                    };
                    let mut app = App::new(&config.nick, &config.host, joins);
                    app.theme = self.theme.theme();
                    app.links = crate::hyperlink::Links::detect(|name| std::env::var(name).ok());
                    if at == 0 {
                        analytics(home, &mut app);
                    }
                    app.use_prefs(crate::prefs::Prefs::load(&prefs));
                    app.site = Some(crate::app::Site {
                        web: self.web_of(home, &org),
                        slug: org.clone(),
                        home: Some(home.to_path_buf()),
                    });
                    // Sealed parts open with the organisation's keys this machine holds.
                    app.chat.random = zirc_core::sealed::fill;
                    let keys = zirc_core::sealed::Keys::new(secrets.as_ref()).of(&org);
                    app.chat.set_keys(&org, keys);
                    // And it's handed the key by a teammate's device when it has none.
                    if let Some(device) = zirc_core::sealed::device_key(secrets.as_ref()) {
                        // Not connected yet: nothing to ask.
                        let _ = app.chat.set_device(device);
                    }
                    orgs.push((org, app, config));
                }
                let zircd = zirc_core::paths::socket(home);
                // Boxed: the client's state makes the future large.
                Box::pin(crate::run(orgs, !self.no_mouse, zircd, prefs)).await
            }
        }
    }

    /// The connection to make, with zirc's directory `home` and the person's `secrets`: the
    /// sign-in's, signing in first if there's none, unless the flags name a server, an
    /// account or settings. Sign-in prompts go to `out`.
    ///
    /// # Errors
    ///
    /// When signing in fails, or as [`Cli::irc_config`].
    pub async fn connection(
        &self,
        home: &Path,
        secrets: &dyn Secrets,
        out: &mut (impl Write + Send),
    ) -> Result<Config> {
        let explicit = self.server.is_some() || self.config.is_some() || self.account.is_some();
        if !explicit {
            if let Some(signin) = signin::load(&signin::path(home))? {
                return Ok(self.finish(signin.connection(secrets)?));
            }
            match signin::sign_in(&self.web, !self.no_browser, home, secrets, out).await {
                Ok(signin) => return Ok(self.finish(signin.connection(secrets)?)),
                Err(err)
                    if matches!(err.downcast_ref(), Some(login::Error::Unavailable { .. })) =>
                {
                    writeln!(
                        out,
                        "{} doesn't take sign-ins: using zirc's settings.",
                        self.web
                    )?;
                }
                Err(err) => return Err(err),
            }
        }
        self.irc_config()
    }

    /// The connections to make, one per organisation signed in to, the first first: every
    /// sign-in's, or as [`Cli::connection`] says when there's none or the flags name a
    /// server, an account or settings.
    ///
    /// # Errors
    ///
    /// As [`Cli::connection`], or when a sign-in's passphrase is gone.
    pub async fn connections(
        &self,
        home: &Path,
        secrets: &dyn Secrets,
        out: &mut (impl Write + Send),
    ) -> Result<Vec<(String, Config)>> {
        let explicit = self.server.is_some() || self.config.is_some() || self.account.is_some();
        if !explicit {
            let signins = signin::load_all(&signin::path(home))?;
            if !signins.is_empty() {
                return signins
                    .iter()
                    .map(|signin| Ok((signin.org(), self.finish(signin.connection(secrets)?))))
                    .collect();
            }
        }
        let config = self.connection(home, secrets, out).await?;
        let server = Server {
            host: config.host.clone(),
            port: Some(config.port),
            tls: None,
            password: None,
        };
        Ok(vec![(zirc_core::settings::org_name(&server), config)])
    }

    /// The zirc.dev the organisation `org` is at: where it was signed in to, or where this
    /// machine logged in, or else the one the flags name.
    fn web_of(&self, home: &Path, org: &str) -> String {
        if let Ok(signins) = signin::load_all(&signin::path(home))
            && let Some(signin) = signins.get(org)
        {
            return signin.web.clone();
        }
        let path = self
            .config
            .clone()
            .unwrap_or_else(|| zirc_core::paths::config(home));
        read_settings(&path)
            .ok()
            .and_then(|settings| settings.web)
            .unwrap_or_else(|| self.web.clone())
    }

    /// The connection the flags and zirc's settings describe.
    ///
    /// # Errors
    ///
    /// When the settings can't be read and no server was given, or a given server's port
    /// isn't a number.
    pub fn irc_config(&self) -> Result<Config> {
        let path = self
            .config
            .clone()
            .unwrap_or_else(|| zirc_core::paths::config(&zirc_core::paths::home()));
        let settings = match (read_settings(&path), &self.server) {
            (Ok(settings), _) => Some(settings),
            (Err(_), Some(_)) => None,
            (Err(err), None) => {
                return Err(
                    err.context("run `zirc login` to set this machine up, or pass --server")
                );
            }
        };
        self.config_from(settings.as_ref())
    }

    fn config_from(&self, settings: Option<&Settings>) -> Result<Config> {
        let server = match (&self.server, settings) {
            (Some(server), _) => {
                let (host, port) = match server.rsplit_once(':') {
                    Some((host, port)) => (
                        host.to_owned(),
                        Some(
                            port.parse()
                                .with_context(|| format!("--server {server}: bad port"))?,
                        ),
                    ),
                    None => (server.clone(), None),
                };
                Server {
                    host,
                    port,
                    tls: None,
                    password: None,
                }
            }
            (None, Some(settings)) => settings.server.clone(),
            (None, None) => bail!("no server: pass --server"),
        };
        let nick = self
            .account
            .clone()
            .or_else(|| settings.and_then(|settings| settings.owner.clone()))
            .or_else(|| std::env::var("USER").ok())
            .unwrap_or_else(|| "zirc".to_owned());
        let mut config = connection(&server, nick);
        config.sasl = self
            .account
            .clone()
            .zip(self.password.clone())
            .map(|(account, password)| Credentials { account, password });
        Ok(self.finish(config))
    }

    /// `config` with what the flags say about any connection, and the capabilities this
    /// client wants.
    fn finish(&self, mut config: Config) -> Config {
        // The person's name when zirc.dev gave one; otherwise the client's.
        if config.realname == config.nick {
            "zirc-tui".clone_into(&mut config.realname);
        }
        if let Some(nick) = &self.nick {
            nick.clone_into(&mut config.nick);
        }
        config.tls &= !self.no_tls;
        config
            .caps
            .extend(EXTRA_CAPS.iter().map(|&cap| cap.to_owned()));
        config
    }
}

/// Starts analytics for the TUI with zirc's directory `home`: says the app opened, sends what's
/// queued in the background, and the first time, tells the person in `app` that it's on.
fn analytics(home: &Path, app: &mut App) {
    use std::io::IsTerminal as _;
    use zirc_core::telemetry::{self, Telemetry};
    use zirc_proto::telemetry::{AppOpened, Client, Event};
    let telemetry = telemetry::init(Telemetry::load(home, Client::Tui));
    telemetry.record(&Event::AppOpened(AppOpened {}));
    let mut notice = Vec::new();
    telemetry.notice(std::io::stdout().is_terminal(), &mut notice);
    if !notice.is_empty() {
        app.note(String::from_utf8_lossy(&notice).trim_end().to_owned());
    }
    tokio::spawn(telemetry::send_every(telemetry, telemetry::EVERY));
}

fn read_settings(path: &Path) -> Result<Settings> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Settings::from_toml(&text).with_context(|| format!("reading {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(args: &[&str]) -> Cli {
        Cli::parse_from(std::iter::once("zirc-tui").chain(args.iter().copied()))
    }

    fn settings() -> Settings {
        Settings::from_toml(
            "nick = \"mattias-mbp\"\nowner = \"mattias\"\n[server]\nhost = \"acme.irc.zirc.dev\"\npassword = \"gate\"\n[account]\nname = \"mattias-mbp\"\npassword = \"machine\"\n",
        )
        .unwrap()
    }

    #[test]
    fn connects_to_the_settings_server_as_the_person() {
        let config = cli(&[]).config_from(Some(&settings())).unwrap();
        assert_eq!(
            (config.host.as_str(), config.port, config.tls),
            ("acme.irc.zirc.dev", 6697, true)
        );
        assert_eq!(config.nick, "mattias");
        assert_eq!(config.server_password.as_deref(), Some("gate"));
        assert!(
            config.sasl.is_none(),
            "the machine's account is the machine's"
        );
        assert!(config.caps.contains(&"echo-message".to_owned()));
    }

    #[test]
    fn flags_override_the_settings() {
        let config = cli(&[
            "--server",
            "127.0.0.1:6667",
            "--no-tls",
            "--account",
            "alice",
            "--password",
            "pw",
        ])
        .config_from(Some(&settings()))
        .unwrap();
        assert_eq!(
            (config.host.as_str(), config.port, config.tls),
            ("127.0.0.1", 6667, false)
        );
        assert_eq!(config.nick, "alice");
        assert_eq!(config.server_password, None);
        assert_eq!(config.sasl.unwrap().account, "alice");
    }

    #[tokio::test]
    async fn connects_as_signed_in_unless_told_otherwise() {
        let home = tempfile::tempdir().unwrap();
        let secrets = zirc_core::secrets::File(home.path().join("secrets.toml"));
        let signin = signin::SignIn {
            handle: "mattias".into(),
            nick: "mattias".into(),
            account: false,
            web: login::WEB.into(),
            server: zirc_core::settings::Server {
                host: "acme.irc.zirc.dev".into(),
                port: None,
                tls: None,
                password: Some("gate".into()),
            },
            realname: None,
            device_key: false,
            media: None,
        };
        signin::save(&signin, &signin::path(home.path())).unwrap();
        let mut out = Vec::new();
        let config = cli(&["--nick", "matt"])
            .connection(home.path(), &secrets, &mut out)
            .await
            .unwrap();
        assert_eq!(
            (config.host.as_str(), config.nick.as_str()),
            ("acme.irc.zirc.dev", "matt")
        );
        assert_eq!(config.server_password.as_deref(), Some("gate"));
        assert!(out.is_empty(), "no sign-in needed");

        let config = cli(&["--server", "127.0.0.1:6667", "--no-tls"])
            .connection(home.path(), &secrets, &mut out)
            .await
            .unwrap();
        assert_eq!((config.host.as_str(), config.tls), ("127.0.0.1", false));
    }

    #[test]
    fn needs_a_server_from_somewhere() {
        let err = cli(&["--config", "/nonexistent/config.toml"])
            .irc_config()
            .unwrap_err();
        assert!(format!("{err:#}").contains("zirc login"), "{err:#}");
        assert!(cli(&["--server", "x:y"]).config_from(None).is_err());
    }
}
