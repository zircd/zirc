//! Sealed parts of messages, for agents and people at a terminal: `zirc team-key`, which keeps
//! this machine's keys for its organisations ([`zirc_core::sealed::Keys`]), `zirc unseal`,
//! which opens a part zircd heard, and the part `zirc post` reads from stdin for `||-||`.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail, ensure};
use clap::{Args, Subcommand};
use zirc_core::ipc::{Request, Response};
use zirc_core::sealed::{Key, Keys, machine_store, new_key};

use crate::args::OrgArg;
use crate::client::{org_here, waits};

/// `zirc team-key`'s subcommands.
#[derive(Debug, Subcommand)]
pub enum TeamKey {
    /// Make a new key for an organisation, which this machine seals with from now on.
    ///
    /// Prints the key to hand to teammates' devices, in person or on a call: never in an
    /// unsealed message. Older keys stay, so what was sealed with them still opens.
    New {
        #[command(flatten)]
        org: OrgArg,
    },
    /// Add the key a teammate handed you, read from stdin.
    ///
    /// Read from stdin, it stays out of shell history.
    Add,
    /// List the keys this machine holds, by organisation and id.
    ///
    /// Never their values.
    List,
    /// Forget one of an organisation's keys on this machine, by the id `list` shows.
    Forget {
        /// The key's id.
        id: String,
        #[command(flatten)]
        org: OrgArg,
    },
}

/// `zirc team-key`.
pub(crate) async fn team_key(
    home: &Path,
    socket: &Path,
    command: TeamKey,
    out: &mut impl Write,
) -> Result<()> {
    let store = machine_store(home);
    let keys = Keys::new(&store);
    match command {
        TeamKey::New { org } => {
            let org = org_named(home, socket, org).await?;
            let key = new_key(org.clone())?;
            keys.add(&key)?;
            writeln!(
                out,
                "made {org}'s key {}, which this machine seals with now. Hand it to your \
                 teammates' devices in person or on a call, never in a message:\n\n{}\n\n\
                 They add it with `zirc team-key add`, or in an app's settings.",
                key.id(),
                key.export()
            )?;
            tell_org(socket, &org, &key, out).await?;
        }
        TeamKey::Add => {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)
                .context("couldn't read the key from stdin")?;
            let key = Key::parse(&text)?;
            let new = keys.add(&key)?;
            let org = key.org();
            if new {
                writeln!(
                    out,
                    "added {org}'s key {}: this machine seals and opens with it",
                    key.id()
                )?;
            } else {
                writeln!(
                    out,
                    "{org}'s key {} was already the one it seals with",
                    key.id()
                )?;
            }
        }
        TeamKey::List => {
            let settings = zirc_core::settings::Settings::of(home);
            let orgs: Vec<String> = settings
                .map(|settings| {
                    settings
                        .organisations()
                        .iter()
                        .map(zirc_core::settings::Organisation::name)
                        .collect()
                })
                .unwrap_or_default();
            let mut any = false;
            for org in &orgs {
                for (at, key) in keys.of(org).iter().enumerate() {
                    any = true;
                    let seals = if at == 0 { ", which seals" } else { "" };
                    writeln!(out, "{org} {}{seals}", key.id())?;
                }
            }
            if !any {
                writeln!(
                    out,
                    "no keys: `zirc team-key add` adds a teammate's, `zirc team-key new` makes \
                     an organisation's first"
                )?;
            }
        }
        TeamKey::Forget { id, org } => {
            let org = org_named(home, socket, org).await?;
            if keys.forget(&org, &id)? {
                writeln!(
                    out,
                    "forgot {org}'s key {id} on this machine: forget it on every other device \
                     too, and change what was sealed with it if someone who left had it"
                )?;
            } else {
                bail!("this machine holds no key {id} for {org}: `zirc team-key list` lists them");
            }
        }
    }
    Ok(())
}

/// Says in `org`'s channel that it has the new key `key`, without it, so teammates know to
/// add it; says here if it couldn't.
async fn tell_org(socket: &Path, org: &str, key: &Key, out: &mut impl Write) -> Result<()> {
    let request = Request::Post {
        repo: crate::client::here(None)?,
        session: crate::args::session_or_user(None),
        text: zirc_core::sealed::new_key_reminder(org, &key.id()),
        to: zirc_core::ipc::Target::Org,
        app: None,
        everyone: false,
        reply: None,
    };
    match crate::client::ask_in(socket, Some(org), request).await {
        Ok(Response::Channel { channel, .. }) => {
            writeln!(out, "told {channel} there's a new key, without it")?;
        }
        Ok(_) | Err(_) => writeln!(
            out,
            "couldn't tell {org}'s channel there's a new key: tell your teammates yourself"
        )?,
    }
    Ok(())
}

/// The organisation `org` names, or else the repository's, or the machine's only one.
async fn org_named(home: &Path, socket: &Path, org: OrgArg) -> Result<String> {
    Ok(crate::search::one_org(home, socket, org.name.as_deref())
        .await?
        .name())
}

/// `zirc unseal`.
#[derive(Debug, Args)]
pub struct UnsealArgs {
    /// The part, as `zirc watch` shows it: `01jbx7ab.1`, a message's id and the part's
    /// number.
    #[arg(value_name = "ID.PART")]
    part: String,
    /// Write it to this file, readable by you only, rather than print it.
    #[arg(long, value_name = "PATH")]
    to: Option<PathBuf>,
    #[command(flatten)]
    org: OrgArg,
}

/// `zirc unseal`: prints only the part's value, without a line break, to redirect.
pub(crate) async fn unseal(socket: &Path, args: UnsealArgs, out: &mut impl Write) -> Result<()> {
    let UnsealArgs { part, to, org } = args;
    let (message, number) = part
        .rsplit_once('.')
        .and_then(|(message, number)| Some((message, number.parse::<usize>().ok()?)))
        .with_context(|| {
            format!(
                "{part} isn't a sealed part: give it as zirc watch shows it, such as 01jbx7ab.1"
            )
        })?;
    let org = org_here(socket, org).await?;
    let request = Request::Unseal {
        message: message.to_owned(),
        part: number,
    };
    let Response::Unsealed { value } = waits(socket, org.as_deref(), request).await? else {
        return Err(crate::client::unexpected());
    };
    match to {
        Some(path) => {
            zirc_core::paths::write_private(&path, &value)
                .with_context(|| format!("couldn't write {}", path.display()))?;
            writeln!(out, "wrote {part} to {}", path.display())?;
        }
        None => write!(out, "{value}")?,
    }
    Ok(())
}

/// What stands in `zirc post`'s words for the part to seal read from stdin.
pub(crate) const FROM_STDIN: &str = "||-||";

/// `text` with [`FROM_STDIN`] replaced by the part `read` gives, between markers, so the part
/// is never in a command line, where `ps` and shell history would see it.
pub(crate) fn with_stdin_part(
    text: String,
    words_from_stdin: bool,
    read: impl FnOnce() -> std::io::Result<String>,
) -> Result<String> {
    if !text.contains(FROM_STDIN) {
        return Ok(text);
    }
    ensure!(
        !words_from_stdin,
        "{FROM_STDIN} reads the part to seal from stdin, where `-` reads the post: give the \
         post's words as arguments"
    );
    ensure!(
        text.matches(FROM_STDIN).count() == 1,
        "{FROM_STDIN} stands for one part only"
    );
    let read = read().context("couldn't read the part to seal from stdin")?;
    let part = read.trim_end_matches(['\n', '\r']);
    ensure!(!part.trim().is_empty(), "nothing to seal on stdin");
    ensure!(
        !part.contains("||") && !part.contains('`'),
        "the part on stdin has || or ` in it, which would end it early"
    );
    Ok(text.replacen(FROM_STDIN, &format!("||{part}||"), 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(part: &str) -> impl FnOnce() -> std::io::Result<String> {
        let part = part.to_owned();
        move || Ok(part)
    }

    #[test]
    fn reads_the_part_for_its_marker_from_stdin() {
        let text = with_stdin_part("staging key: ||-||".into(), false, read("sk_1\n")).unwrap();
        assert_eq!(text, "staging key: ||sk_1||");
        // Without the marker, stdin isn't read.
        let unread = || -> std::io::Result<String> { panic!("read stdin") };
        assert_eq!(
            with_stdin_part("no part".into(), false, unread).unwrap(),
            "no part"
        );
    }

    #[test]
    fn refuses_what_it_cant_read_one_part_from() {
        for (text, from_stdin, part) in [
            ("a ||-||", true, "sk_1"),
            ("||-|| and ||-||", false, "sk_1"),
            ("a ||-||", false, "\n"),
            ("a ||-||", false, "sk||1"),
        ] {
            assert!(
                with_stdin_part(text.into(), from_stdin, read(part)).is_err(),
                "{text} {part:?}"
            );
        }
    }
}
