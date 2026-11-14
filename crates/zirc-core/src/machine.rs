//! This machine, as zirc names it to others.

use std::io::Write as _;
use std::path::Path;
use std::process::Command;

/// The machine's name, as the person named it, such as `matt` or `Mattias's MacBook Pro`,
/// or empty when it can't be found: on a Mac, its computer name (System Settings, Sharing),
/// then its local host name; on Linux, its pretty host name, then its static one; then, and
/// elsewhere, `hostname -s`. zirc.dev makes it a machine name, `matt` or
/// `mattias-s-macbook-pro`. Every login asks this, so a machine is named the same whichever
/// signs it in: `hostname -s` alone can be what the network called it, such as `Mac`.
#[must_use]
pub fn host_name() -> String {
    let named = if cfg!(target_os = "macos") {
        output("scutil", &["--get", "ComputerName"])
            .or_else(|| output("scutil", &["--get", "LocalHostName"]))
    } else if cfg!(target_os = "linux") {
        output("hostnamectl", &["--pretty"]).or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .and_then(|name| some(name.trim()))
        })
    } else {
        None
    };
    named
        .or_else(|| output("hostname", &["-s"]))
        .unwrap_or_default()
}

/// What `program` prints, trimmed, when it succeeds and prints something.
fn output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    some(String::from_utf8(output.stdout).ok()?.trim())
}

fn some(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_owned())
}

/// This machine's own random id, made the first time it's needed and kept in zirc's
/// directory `home`: how zirc.dev knows it's the same machine logging in again, whatever
/// it's called now, and doesn't let it in twice under two names.
#[must_use]
pub fn id(home: &Path) -> Option<String> {
    std::fs::create_dir_all(home).ok()?;
    random_id_in(&home.join("machine-id"))
}

/// The random id kept at `path`, made there the first time.
pub(crate) fn random_id_in(path: &Path) -> Option<String> {
    let read = || {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|id| some(id.trim()))
    };
    if let Some(id) = read() {
        return Some(id);
    }
    let id = random_id()?;
    // Made only if no other program made it meanwhile; then that one's is the id.
    let made = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| file.write_all(id.as_bytes()));
    match made {
        Ok(()) => Some(id),
        Err(_) => read(),
    }
}

/// A random version 4 UUID.
fn random_id() -> Option<String> {
    use ring::rand::SecureRandom as _;
    let mut bytes = [0u8; 16];
    ring::rand::SystemRandom::new().fill(&mut bytes).ok()?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = zirc_proto::hex(&bytes);
    Some(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_has_a_name() {
        assert!(!host_name().is_empty());
    }

    #[test]
    fn a_machines_id_is_made_once_and_kept() {
        let home = tempfile::tempdir().unwrap();
        let first = id(home.path()).unwrap();
        assert_eq!(first.len(), 36);
        assert_eq!(id(home.path()), Some(first.clone()));
        let other = tempfile::tempdir().unwrap();
        assert_ne!(id(other.path()), Some(first));
    }
}
