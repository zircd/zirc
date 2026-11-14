//! `zirc login acme/web` (`zirc logout acme/web` is in `logout.rs`): this machine as a guest's terminal in a
//! public channel ([`zirc_proto::guest_terminal`]), so an agent here talks in its guest's
//! room there as the guest does in the browser.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use zirc_core::device_key::{DeviceKey as _, Software};
use zirc_core::ipc::{Request, Response};
use zirc_core::login::Client;
use zirc_core::paths;
use zirc_proto::PROGRAM;
use zirc_proto::guest_terminal::{self, GuestTarget, Target};
use zirc_proto::login::LoginRequest;

use crate::client::{ask, shown};
use crate::daemon::{start, stop, zircd_binary};
use crate::service;
use crate::setup::{Login, connected_to, earlier_setup, login_settings};

/// The public channel `text` names, as `zirc login` takes it.
pub fn target(text: &str) -> Result<Target> {
    guest_terminal::parse_target(text).map_err(|why| anyhow::anyhow!("{text:?}: {why}"))
}

/// The organisation `text` names, as `zirc login` took it (acme/web) or by itself (acme):
/// what `zirc post --in` and `zirc logout` take.
pub fn org_of(text: &str) -> String {
    target(text).map_or_else(|_| text.trim().to_owned(), |it| it.org)
}

/// `zirc login <target>`: connects this machine to a public channel as the guest who lets it
/// in, from that channel's chat, and saves it as an organisation named the organisation's.
pub async fn login(home: &Path, args: Login, target: Target, out: &mut impl Write) -> Result<()> {
    let web = target.web.clone().unwrap_or_else(|| args.web.clone());
    let path = paths::config(home);
    let earlier = earlier_setup(&path, false)?;
    if let Some(own) = earlier
        .as_ref()
        .and_then(|settings| settings.organisation(&target.org))
        .filter(|org| org.guest.is_none())
    {
        bail!(
            "this machine is in {} already, as one of its own: it can't be a guest there too",
            own.name()
        );
    }
    let key = Software::machine(home);
    let device_key = key.public_key().context(
        "this machine's key, which a guest's terminal proves itself with, couldn't be made",
    )?;
    let request = LoginRequest {
        client: format!("{PROGRAM} {}", env!("CARGO_PKG_VERSION")),
        host: guest_terminal::machine_name(&zirc_core::machine::host_name()),
        kind: None,
        orgs: None,
        org: None,
        machine_id: zirc_core::machine::id(home),
        device_key: Some(device_key),
        connected: None,
        guest: Some(GuestTarget {
            org: target.org.clone(),
            channel: target.channel.clone(),
        }),
    };
    let client = Client::new(&web)?;
    let started = client.start(&request).await?;
    writeln!(
        out,
        "Open this link to let this machine into {}/{} as you:",
        target.org,
        target.channel.trim_start_matches('#')
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
    let credentials = client.wait(&started).await?;
    let org = zirc_core::guest::organisation(&credentials, &web)
        .with_context(|| format!("{web} answered with no guest's terminal: nothing changed"))?;
    let name = org.name();
    let of = org
        .guest
        .as_ref()
        .map(zirc_core::guest::of)
        .unwrap_or_default();
    let settings = login_settings(std::slice::from_ref(&org), earlier.as_ref(), false);
    settings.save(&path)?;
    writeln!(out)?;
    writeln!(
        out,
        "Logged in to {of} as a guest, as set in {}",
        shown(&path)
    )?;
    // No skill and no login service: a terminal lasts as long as its guest, and zircd takes it
    // off this machine once zirc.dev forgets them, leaving nothing behind.
    if args.no_daemon {
        writeln!(
            out,
            "zircd isn't started: run it at login with `zirc daemon install`, or until you stop \
             it with `zirc daemon start`"
        )?;
        return Ok(());
    }
    run_zircd(home, args.zircd, out).await?;
    connected_to(home, Some(&name), out).await
}

/// Starts zircd with a new guest's terminal: restarting the login service's, when this
/// machine's own organisations run it at login, or else one of its own until the computer
/// restarts, from the binary `zircd` names.
async fn run_zircd(home: &Path, zircd: Option<PathBuf>, out: &mut impl Write) -> Result<()> {
    let socket = paths::socket(home);
    if service::installed_for(home) {
        writeln!(
            out,
            "Restarting zircd with it, and waiting for it to connect…"
        )?;
        if ask(&socket, Request::Restart).await.is_err() {
            service::start()?;
        }
    } else {
        writeln!(
            out,
            "Starting zircd until this computer restarts, and waiting for it to connect…"
        )?;
        let _ = stop(&socket, &mut std::io::sink()).await;
        start(home, &zircd_binary(zircd)?, &mut std::io::sink()).await?;
    }
    Ok(())
}

/// The words `zirc status` gives a guest's terminal's connection, after its organisation's
/// name: `connected to acme/web as guest-21042-mbp`.
pub fn status_line(connected: bool, problem: bool, of: &str, nick: Option<&str>) -> String {
    let state = if connected {
        "connected"
    } else if problem {
        "not connecting"
    } else {
        "connecting"
    };
    let nick = nick.map(|nick| format!(" as {nick}")).unwrap_or_default();
    format!("{state} to {of}{nick}")
}

/// What `zirc login` says once a guest's terminal is connected.
pub fn connected_lines(status: &Response, name: &str) -> Option<Vec<String>> {
    let Response::Status(status) = status else {
        return None;
    };
    let (connected, nick, of) = match status.orgs.iter().find(|org| org.name == name) {
        Some(org) => (org.connected, org.nick.clone(), org.guest.clone()),
        None => (status.connected, status.nick.clone(), status.guest.clone()),
    };
    let of = of?;
    // In an organisation of its own too, a post goes there unless it says where.
    let post = if status.orgs.iter().any(|org| org.guest.is_none()) {
        format!("zirc post --in {of} …")
    } else {
        "zirc post …".to_owned()
    };
    connected.then(|| {
        vec![
            format!(
                "zircd is running, connected to {of} as {}.",
                nick.unwrap_or_default()
            ),
            String::new(),
            format!(
                "Your agents here hear what's said in your room with `zirc watch`, and say \
                 something there with `{post}`."
            ),
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zirc_core::ipc::{OrgStatus, Status};

    #[test]
    fn a_target_is_a_public_channel() {
        let acme = target("acme/web").unwrap();
        assert_eq!((acme.org.as_str(), acme.channel.as_str()), ("acme", "#web"));
        assert!(acme.web.is_none());
        let local = target("http://localhost:3000/chat?org=acme&channel=%23web&guest").unwrap();
        assert_eq!(local.web.as_deref(), Some("http://localhost:3000"));
        assert_eq!(org_of("acme/web"), "acme");
        assert_eq!(org_of(" acme "), "acme");
        let refused = target("acme").unwrap_err().to_string();
        assert!(refused.contains("such as acme/web"), "{refused}");
    }

    #[test]
    fn a_terminal_alone_steps_aside_for_the_machines_own_organisation() {
        let terminal = zirc_core::settings::Organisation {
            name: Some("acme".into()),
            nick: None,
            server: zirc_core::settings::Server {
                host: "acme.irc.zirc.dev".into(),
                port: None,
                tls: None,
                password: None,
            },
            account: None,
            owner: None,
            web: Some("https://zirc.dev".into()),
            guest: Some(zirc_core::settings::GuestCredentials {
                url: "wss://acme.irc.zirc.dev/guest".into(),
                org: "acme".into(),
                guest: "guest-21042".into(),
                channel: "#web/guest-21042".into(),
                public: "#web".into(),
                grant: "g".into(),
                expires: 9,
            }),
        };
        let alone = login_settings(std::slice::from_ref(&terminal), None, false);
        assert_eq!(alone.organisations()[0].name(), "acme");
        assert!(alone.guest.is_some());

        let globex = zirc_core::settings::Organisation {
            name: None,
            web: None,
            guest: None,
            server: zirc_core::settings::Server {
                host: "globex.irc.zirc.dev".into(),
                ..terminal.server.clone()
            },
            ..terminal.clone()
        };
        let both = login_settings(&[globex], Some(&alone), false);
        let names: Vec<String> = both
            .organisations()
            .iter()
            .map(zirc_core::settings::Organisation::name)
            .collect();
        assert_eq!(names, ["globex", "acme"]);
        assert_eq!(both.organisation("acme"), Some(terminal));
    }

    #[test]
    fn says_whose_guest_it_is() {
        assert_eq!(
            status_line(true, false, "acme/web", Some("guest-21042-mbp")),
            "connected to acme/web as guest-21042-mbp"
        );
        assert_eq!(
            status_line(false, true, "acme/web", None),
            "not connecting to acme/web"
        );
    }

    #[test]
    fn says_it_connected_as_its_guests_terminal() {
        let org = |name: &str, guest: Option<&str>| OrgStatus {
            name: name.into(),
            connected: true,
            server: "acme.irc.zirc.dev:443".into(),
            nick: Some("guest-21042-mbp".into()),
            channels: vec!["#web/guest-21042".into()],
            problem: None,
            guest: guest.map(str::to_owned),
        };
        let status = Response::Status(Status {
            connected: true,
            server: "globex.irc.zirc.dev:6697".into(),
            nick: Some("m".into()),
            channels: Vec::new(),
            problem: None,
            watching: Vec::new(),
            protocol: zirc_core::ipc::PROTOCOL,
            version: None,
            orgs: vec![org("globex", None), org("acme", Some("acme/web"))],
            breaker: None,
            guest: None,
            busy_ms: None,
        });
        let lines = connected_lines(&status, "acme").unwrap();
        assert_eq!(
            lines[0],
            "zircd is running, connected to acme/web as guest-21042-mbp."
        );
        assert!(
            lines[2].contains("`zirc post --in acme/web …`"),
            "{lines:?}"
        );
        assert!(connected_lines(&status, "globex").is_none());
        // A guest's terminal alone takes every post: no --in.
        let Response::Status(mut alone) = status else {
            unreachable!()
        };
        alone.orgs.remove(0);
        let lines = connected_lines(&Response::Status(alone), "acme").unwrap();
        assert!(lines[2].contains("`zirc post …`"), "{lines:?}");
    }
}
