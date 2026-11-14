//! Running zircd at login: a launchd agent on macOS, a systemd user service on Linux. Both
//! restart zircd if it fails, but not once it's stopped (`zirc daemon stop`), and both keep
//! it apart from any shell.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use zirc_proto::PROGRAM;

use anyhow::{Context as _, Result, bail};

/// launchd's name for the agent.
const LABEL: &str = "dev.zirc.zircd";
/// systemd's name for the service.
const UNIT: &str = "zircd.service";
/// Seconds between restarts of a zircd that keeps failing, as with a config.toml it can't
/// read, so its log grows by a line or two a minute, not every second.
const THROTTLE: u32 = 30;

/// Installs and starts the service for this platform.
pub fn install(home: &Path, zircd: &Path, out: &mut impl Write) -> Result<()> {
    let zircd = zircd
        .canonicalize()
        .with_context(|| format!("finding {}", zircd.display()))?;
    let path = service_file()?;
    std::fs::create_dir_all(path.parent().expect("a service file has a directory"))?;
    if cfg!(target_os = "macos") {
        std::fs::write(&path, launchd_plist(&zircd, home))?;
        // Replace an agent loaded earlier, then load this one.
        let _ = launchctl(&["bootout", &gui()?, &path.to_string_lossy()]);
        launchctl(&["bootstrap", &gui()?, &path.to_string_lossy()])?;
    } else {
        std::fs::write(&path, systemd_unit(&zircd, home))?;
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", "--now", UNIT])?;
    }
    writeln!(out, "installed {}; zircd now runs at login", path.display())?;
    Ok(())
}

/// Stops the service and removes it.
pub fn uninstall(out: &mut impl Write) -> Result<()> {
    let path = service_file()?;
    if !path.exists() {
        writeln!(out, "zircd isn't installed")?;
        return Ok(());
    }
    if cfg!(target_os = "macos") {
        launchctl(&["bootout", &gui()?, &path.to_string_lossy()])?;
        std::fs::remove_file(&path)?;
    } else {
        systemctl(&["disable", "--now", UNIT])?;
        std::fs::remove_file(&path)?;
        systemctl(&["daemon-reload"])?;
    }
    writeln!(out, "removed {}", path.display())?;
    Ok(())
}

fn service_file() -> Result<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME isn't set")?);
    Ok(if cfg!(target_os = "macos") {
        home.join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map_or_else(|| home.join(".config"), PathBuf::from)
            .join("systemd/user")
            .join(UNIT)
    })
}

fn launchd_plist(zircd: &Path, home: &Path) -> String {
    // launchd appends zircd's output to it and never rotates it: zircd keeps it small itself
    // (zircd's `log` module).
    let log = zirc_core::paths::log(home);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{zircd}</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>ZIRC_HOME</key>
        <string>{home}</string>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ThrottleInterval</key>
    <integer>{THROTTLE}</integer>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#,
        zircd = xml(&zircd.to_string_lossy()),
        home = xml(&home.to_string_lossy()),
        log = xml(&log.to_string_lossy()),
    )
}

fn systemd_unit(zircd: &Path, home: &Path) -> String {
    format!(
        "[Unit]\n\
         Description={PROGRAM} daemon: one IRC connection for this machine\n\
         \n\
         [Service]\n\
         ExecStart=\"{zircd}\"\n\
         Environment=\"ZIRC_HOME={home}\"\n\
         Restart=on-failure\n\
         RestartSec={THROTTLE}\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        // ExecStart expands $VARIABLES, Environment doesn't; both expand %specifiers.
        zircd = systemd_quoted(&zircd.to_string_lossy()).replace('$', "$$"),
        home = systemd_quoted(&home.to_string_lossy()),
    )
}

/// `text` for inside a systemd unit's double quotes.
fn systemd_quoted(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%")
}

/// Whether the service installed for this platform runs zircd for `home`, so that starting
/// zircd is the service manager's to do.
pub fn installed_for(home: &Path) -> bool {
    let Ok(path) = service_file() else {
        return false;
    };
    let Ok(installed) = std::fs::read_to_string(path) else {
        return false;
    };
    let ours = if cfg!(target_os = "macos") {
        format!(
            "<key>ZIRC_HOME</key>\n        <string>{}</string>",
            xml(&home.to_string_lossy())
        )
    } else {
        format!(
            "Environment=\"ZIRC_HOME={}\"",
            systemd_quoted(&home.to_string_lossy())
        )
    };
    installed.contains(&ours)
}

/// Starts the installed service's zircd, as after `zirc daemon stop`: a zircd that exits
/// cleanly stays stopped until then, or the next login.
pub fn start() -> Result<()> {
    if cfg!(target_os = "macos") {
        let path = service_file()?;
        // Loaded since login, unless something unloaded it.
        if launchctl(&["kickstart", &format!("{}/{LABEL}", gui()?)]).is_err() {
            launchctl(&["bootstrap", &gui()?, &path.to_string_lossy()])?;
        }
        Ok(())
    } else {
        systemctl(&["start", UNIT])
    }
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// launchd's domain for the logged-in user, `gui/<uid>`.
fn gui() -> Result<String> {
    let uid = Command::new("id").arg("-u").output()?;
    Ok(format!("gui/{}", String::from_utf8(uid.stdout)?.trim()))
}

fn launchctl(args: &[&str]) -> Result<()> {
    run("launchctl", args)
}

fn systemctl(args: &[&str]) -> Result<()> {
    let mut all = vec!["--user"];
    all.extend(args);
    run("systemctl", &all)
}

fn run(program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program).args(args).output()?;
    if !output.status.success() {
        bail!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launchd_keeps_zircd_running_with_its_home() {
        let plist = launchd_plist(
            Path::new("/opt/zirc/bin/zircd"),
            Path::new("/Users/me/.zirc"),
        );
        assert!(plist.contains("<string>/opt/zirc/bin/zircd</string>"));
        assert!(plist.contains("<key>ZIRC_HOME</key>\n        <string>/Users/me/.zirc</string>"));
        // Restarted when it fails, a little apart, but stopped when it's stopped.
        assert!(plist.contains(
            "<key>KeepAlive</key>\n    <dict>\n        <key>SuccessfulExit</key>\n        <false/>"
        ));
        assert!(plist.contains("<key>ThrottleInterval</key>\n    <integer>30</integer>"));
        assert!(plist.contains("<string>/Users/me/.zirc/zircd.log</string>"));
    }

    #[test]
    fn escapes_paths_in_the_plist() {
        let plist = launchd_plist(Path::new("/a&b/<zircd>"), Path::new("/h"));
        assert!(plist.contains("/a&amp;b/&lt;zircd&gt;"));
    }

    #[test]
    fn systemd_restarts_zircd_with_its_home() {
        let unit = systemd_unit(
            Path::new("/usr/local/bin/zircd"),
            Path::new("/home/me/.zirc"),
        );
        assert!(unit.contains("ExecStart=\"/usr/local/bin/zircd\""));
        assert!(unit.contains("Environment=\"ZIRC_HOME=/home/me/.zirc\""));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("RestartSec=30"));
    }

    #[test]
    fn escapes_paths_in_the_unit() {
        let unit = systemd_unit(
            Path::new(r#"/opt/100%/"q"\$x/zircd"#),
            Path::new("/home/50%/.zirc"),
        );
        assert!(
            unit.contains(r#"ExecStart="/opt/100%%/\"q\"\\$$x/zircd""#),
            "{unit}"
        );
        assert!(
            unit.contains(r#"Environment="ZIRC_HOME=/home/50%%/.zirc""#),
            "{unit}"
        );
    }
}
