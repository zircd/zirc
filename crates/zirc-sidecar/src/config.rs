//! Ergo's config file for a server ([`file`]), and putting it and the MOTD in force, in the
//! sidecar beside Ergo: the controller holds a server's settings to its plan, renders both
//! from them and keeps them in the tenant's `ConfigMap`, which Ergo's
//! pod starts from, and gives them to the sidecar, which writes them over Ergo's and has it
//! read them again (`REHASH`) there and then. What Ergo can't change while it runs (the line
//! length) the sidecar leaves as it is, and says it waits for a restart.

use std::path::Path;

use serde_yaml::Value;
use zirc_proto::desired::{ChannelCreation, Multiline, RateLimit, Settings};

use crate::ergo::{Operator, OperatorError};

/// The config file's and the MOTD's names, in the `ConfigMap` and beside Ergo's database.
pub const CONFIG: &str = "ircd.yaml";
/// The MOTD's name, likewise.
pub const MOTD: &str = "ircd.motd";

/// A setting Ergo can change only on starting, by its key in the file and its field.
const ON_RESTART: &[(&[&str], &str)] = &[(&["server", "max-line-len"], "settings.lineLength")];

/// `yaml` with the value at `path` set to `value`, a YAML scalar or flow sequence, and the
/// rest as it was, comments included: the release's `default.yaml` is YAML 1.1, which
/// `serde_yaml` would change as it wrote it out again (`0777`, an octal number there, becomes a
/// string). Keys are indented four spaces a level, as Ergo's file has them; a key that isn't
/// there (or is only in a comment) goes first in its section. `None` when a section on the
/// way isn't there.
#[must_use]
pub fn set(yaml: &str, path: &[&str], value: &str) -> Option<String> {
    put(yaml, path, value, false)
}

/// As [`set`], and what was nested under the key goes too: its block value, such as the
/// release's example under `accounts.jwt-auth.tokens`, comments in it included.
#[must_use]
pub fn replace(yaml: &str, path: &[&str], value: &str) -> Option<String> {
    put(yaml, path, value, true)
}

/// `yaml` with Ergo's `accounts.jwt-auth` for the browser client's tokens' `keys`
/// ([`crate::environment::token_auth_settings`]), the release's example token gone: in the
/// file, which Ergo reads again ([`apply`]) as keys come and go, where its environment would
/// restart it. As it was without keys. `None` when `yaml` has no `accounts.jwt-auth`.
#[must_use]
pub fn with_token_auth(yaml: &str, keys: &[String]) -> Option<String> {
    crate::environment::token_auth_settings(keys)
        .iter()
        .try_fold(yaml.to_owned(), |yaml, (path, value)| {
            replace(&yaml, path, value)
        })
}

fn put(yaml: &str, path: &[&str], value: &str, nested_too: bool) -> Option<String> {
    let mut lines: Vec<String> = yaml.lines().map(str::to_owned).collect();
    let key_at = |line: &str, depth: usize, key: &str| {
        let indent = depth * 4;
        line.get(..indent)
            .is_some_and(|lead| lead.bytes().all(|b| b == b' '))
            && line[indent..]
                .strip_prefix(key)
                .is_some_and(|rest| rest.starts_with(':'))
    };
    // The lines that belong to the section at `depth` starting after `from`.
    let section_end = |lines: &[String], from: usize, depth: usize| {
        lines[from..]
            .iter()
            .position(|line| {
                let text = line.trim_start();
                !text.is_empty() && !text.starts_with('#') && line.len() - text.len() < depth * 4
            })
            .map_or(lines.len(), |at| from + at)
    };
    let (last, parents) = path.split_last()?;
    let (mut from, mut to) = (0, lines.len());
    for (depth, key) in parents.iter().enumerate() {
        let at = (from..to).find(|&i| key_at(&lines[i], depth, key))?;
        from = at + 1;
        to = section_end(&lines, from, depth + 1);
    }
    let depth = parents.len();
    let line = format!("{}{last}: {value}", " ".repeat(depth * 4));
    match (from..to).find(|&i| key_at(&lines[i], depth, last)) {
        Some(at) => {
            lines[at] = line;
            if nested_too {
                let nested = lines[at + 1..]
                    .iter()
                    .take_while(|l| {
                        !l.trim().is_empty() && l.len() - l.trim_start().len() > depth * 4
                    })
                    .count();
                lines.drain(at + 1..at + 1 + nested);
            }
        }
        None => lines.insert(from, line),
    }
    Some(lines.join("\n") + "\n")
}

/// `yaml`, Ergo's config file, without its `server.listeners` block. Ergo merges an
/// environment override into the file's listeners rather than replacing them, so the
/// release's (6667 on loopback, and a TLS 6697) would otherwise stay: whoever runs Ergo with
/// listeners of its own (a self-hosted server, tests on free ports) gives all of them.
///
/// # Panics
///
/// When the block isn't where Ergo's `default.yaml` keeps it.
#[must_use]
pub fn without_listeners(yaml: &str) -> String {
    let (start, end) = listeners(yaml).expect("no `listeners:` in Ergo's config");
    let lines: Vec<&str> = yaml.lines().collect();
    [&lines[..start], &lines[end..]].concat().join("\n") + "\n"
}

/// Where `yaml`'s `server.listeners` block is, as a range of lines: from its key until the
/// next key at the same depth.
fn listeners(yaml: &str) -> Option<(usize, usize)> {
    const START: &str = "    listeners:";
    let lines: Vec<&str> = yaml.lines().collect();
    let start = lines.iter().position(|line| *line == START)?;
    let is_inside = |line: &&str| {
        line.trim().is_empty() || line.trim_start().starts_with('#') || line.starts_with("     ")
    };
    let end = start
        + 1
        + lines[start + 1..]
            .iter()
            .take_while(|line| is_inside(line))
            .count();
    Some((start, end))
}

/// `config` listening as `running` does: the listeners are the server's own, not a setting
/// (a self-hosted server gives its own in Ergo's environment and its file has none, and one
/// behind a proxy that ends TLS has no certificate for the release's 6697), so `running`'s
/// block, or none, takes the place of `config`'s.
fn listening_as(running: &str, config: &str) -> String {
    let Some((start, end)) = listeners(config) else {
        return config.to_owned();
    };
    let lines: Vec<&str> = config.lines().collect();
    let theirs: Vec<&str> = listeners(running)
        .map(|(from, to)| running.lines().skip(from).take(to - from).collect())
        .unwrap_or_default();
    [&lines[..start], &theirs[..], &lines[end..]]
        .concat()
        .join("\n")
        + "\n"
}

/// Ergo's `history.restrictions.expire-time` for history that never expires: at zero its
/// SQLite store deletes nothing (`irc/sqlite`'s `cleanupLoop`) and every message stays
/// retrievable. What a server keeps when neither its owners, an operator nor its plan bound it.
pub const KEEP_EVERYTHING: &str = "0";

/// How many connections one client address may have, as Envoy's PROXY header gives it (the
/// edge can't limit IRC connections itself): enough for a team behind one office's NAT, each
/// person with a desktop, a phone and agent sessions, but not for one address to take the
/// server. In the file because Ergo can't take them from its environment: `server.ip-limits`
/// embeds an unexported struct, which its overrides can't reach, so they never applied and
/// Ergo's defaults did (16 at once, 32 new in 10 minutes). The guest gateway connects from
/// the pod's loopback, which they exempt, and limits its guests itself (`crate::gateway`).
const IP_LIMITS: &[(&[&str], &str)] = &[
    (&["server", "ip-limits", "count"], "true"),
    (
        &["server", "ip-limits", "max-concurrent-connections"],
        "1024",
    ),
    (&["server", "ip-limits", "throttle"], "true"),
    (&["server", "ip-limits", "window"], "1m"),
    (
        &["server", "ip-limits", "max-connections-per-window"],
        "600",
    ),
];

/// What a server's settings come to once whoever runs it has held them to its bounds: zirc's
/// controller to the organisation's plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    /// How long history is kept, as Ergo's `expire-time` has it (`30d`, or
    /// [`KEEP_EVERYTHING`]).
    pub retention: String,
    /// The longest line, in bytes.
    pub line_length: u32,
    /// How long a message of several lines may be.
    pub multiline: Multiline,
    /// How fast a connection may send.
    pub rate: RateLimit,
}

/// Ergo's config file for a server: the release's `default.yaml` with the server's
/// `settings` in it, as `held`.
///
/// # Panics
///
/// When `default_yaml` isn't an Ergo release's, with the sections set here.
#[must_use]
pub fn file(default_yaml: &str, settings: &Settings, held: &Held) -> String {
    let Held {
        retention,
        line_length,
        multiline,
        rate,
    } = held.clone();
    let channels: Vec<String> = settings
        .default_channels
        .iter()
        .map(|c| c.to_lowercase())
        .collect();
    let operators = settings.channel_creation == ChannelCreation::Operators;
    [
        (&["history", "restrictions", "expire-time"][..], retention),
        (
            &["channels", "operator-only-creation"],
            operators.to_string(),
        ),
        // JSON is YAML, and quotes each name, which starts with a #.
        (
            &["channels", "auto-join"],
            serde_json::json!(channels).to_string(),
        ),
        (&["server", "max-line-len"], line_length.to_string()),
        (
            &["limits", "multiline", "max-bytes"],
            multiline.max_bytes.to_string(),
        ),
        (
            &["limits", "multiline", "max-lines"],
            multiline.max_lines.to_string(),
        ),
        (&["fakelag", "window"], "1s".into()),
        (&["fakelag", "burst-limit"], rate.burst.to_string()),
        (
            &["fakelag", "messages-per-window"],
            rate.per_second.to_string(),
        ),
    ]
    .into_iter()
    // Ergo keeps names like its guest template (`Guest-*`) from being registered: zirc.dev's
    // guests' accounts are `guest-…` (`crate::gateway`), so it gets one of its own, which it
    // only gives clients it has to rename.
    .chain([(
        &["accounts", "nick-reservation", "guest-nickname-format"][..],
        "\"Unregistered-*\"".to_owned(),
    )])
    .chain(
        IP_LIMITS
            .iter()
            .map(|(path, value)| (*path, (*value).to_owned())),
    )
    .fold(
        as_the_image_has_it(default_yaml),
        |config, (path, value)| set(&config, path, &value).expect("default.yaml has the section"),
    )
}

/// The release's `default.yaml` as its image (`/ircd-bin/default.yaml`) has it: the image's
/// Dockerfile listens on `:6667` in place of `127.0.0.1:6667`, and not on `[::1]:6667`, so
/// that the port is reachable from outside the container. Ergo's environment adds to the
/// file's listeners rather than replacing them, so they have to be the
/// image's: with the release's, Ergo binds 127.0.0.1:6667 twice and won't start.
fn as_the_image_has_it(default_yaml: &str) -> String {
    default_yaml
        .lines()
        .filter(|line| !line.trim_start().starts_with("\"[::1]:6667\":"))
        .map(
            |line| match line.trim_start().strip_prefix("\"127.0.0.1:6667\":") {
                Some(_) => format!(
                    "{}\":6667\":",
                    &line[..line.len() - line.trim_start().len()]
                ),
                None => line.to_owned(),
            },
        )
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

#[derive(Debug, thiserror::Error)]
/// Why a config couldn't be put in force.
pub enum ApplyError {
    #[error("not YAML: {0}")]
    /// The config isn't YAML.
    Yaml(#[from] serde_yaml::Error),
    #[error("couldn't write Ergo's config: {0}")]
    /// The config file or the MOTD couldn't be written.
    Write(#[from] std::io::Error),
    #[error("the config has no section for {0}")]
    /// The config has no section for a setting.
    Missing(&'static str),
    #[error(transparent)]
    /// Ergo refused to read the config again.
    Ergo(#[from] OperatorError),
}

/// Puts `config` and `motd` in force on the Ergo beside the sidecar, whose config file and
/// MOTD are in `dir`: writes them there, listening as it does already ([`listening_as`]), and
/// has Ergo read them again. A setting Ergo can't
/// change while it runs stays as it started, in the file too (else it couldn't read it
/// again), and comes back among those that wait for a restart; the pod starts from the
/// `ConfigMap`, which has it.
///
/// # Errors
///
/// When the config isn't YAML, the files can't be written, or Ergo keeps its old config,
/// with what it said.
pub async fn apply(
    dir: &Path,
    operator: &Operator,
    config: &str,
    motd: &str,
) -> Result<Vec<String>, ApplyError> {
    let path = dir.join(CONFIG);
    let running_text = std::fs::read_to_string(&path)?;
    let running: Value = serde_yaml::from_str(&running_text)?;
    let mut config = listening_as(&running_text, config);
    let mut pending = Vec::new();
    for (keys, field) in ON_RESTART {
        let get = |value: &Value| keys.iter().try_fold(value, |at, key| at.get(*key)).cloned();
        let now = get(&running);
        if get(&serde_yaml::from_str(&config)?) != now {
            pending.push((*field).to_owned());
            // Absent, it's Ergo's default; every config the controller renders says it.
            let now = now.map_or_else(|| "512".to_owned(), |now| yaml_scalar(&now));
            config = set(&config, keys, &now).ok_or(ApplyError::Missing(field))?;
        }
    }
    write(&path, &config)?;
    write(&dir.join(MOTD), motd)?;
    operator.rehash().await?;
    Ok(pending)
}

/// A scalar as YAML has it.
fn yaml_scalar(value: &Value) -> String {
    serde_yaml::to_string(value)
        .unwrap_or_default()
        .trim_end()
        .to_owned()
}

/// Writes `text` to `path` whole: Ergo never reads half a file.
fn write(path: &Path, text: &str) -> std::io::Result<()> {
    let partial = path.with_extension("partial");
    std::fs::write(&partial, text)?;
    std::fs::rename(partial, path)
}

#[cfg(test)]
mod tests {
    #[test]
    fn drops_only_the_listeners_block() {
        let yaml = "server:\n    name: x\n    listeners:\n        # tls\n        \":6697\":\n            tls: {}\n\n    max-sendq: 96k\nnext:\n";
        assert_eq!(
            super::without_listeners(yaml),
            "server:\n    name: x\n    max-sendq: 96k\nnext:\n"
        );
    }

    use super::*;

    #[test]
    fn a_config_put_in_force_listens_as_the_server_does_already() {
        let given = "server:\n    name: x\n    listeners:\n        \":6667\":\n        \":6697\":\n            tls:\n                cert: fullchain.pem\n    max-sendq: 96k\nnext:\n";
        // A self-hosted server's file has none: its listeners are in Ergo's environment, and
        // one behind a proxy that ends TLS has no certificate for 6697.
        let own = "server:\n    name: x\n    max-sendq: 96k\nnext:\n";
        assert_eq!(listening_as(own, given), own);
        // A tenant's, or any with listeners of its own in the file, keeps them.
        let running = "server:\n    name: x\n    listeners:\n        \":6667\":\n            proxy: true\n    max-sendq: 64k\n";
        assert_eq!(
            listening_as(running, given),
            "server:\n    name: x\n    listeners:\n        \":6667\":\n            proxy: true\n    max-sendq: 96k\nnext:\n"
        );
        // A config with no block of its own is left as it is.
        assert_eq!(listening_as(running, own), own);
    }

    #[test]
    fn set_finds_a_key_by_its_section_and_adds_one_that_isn_t_there() {
        let yaml =
            "a:\n    window: 10m\n    #b: 1\nb:\n    window: 1s # comment\n    c:\n        d: 1\n";
        assert_eq!(
            set(yaml, &["b", "window"], "2s").unwrap(),
            "a:\n    window: 10m\n    #b: 1\nb:\n    window: 2s\n    c:\n        d: 1\n"
        );
        assert_eq!(
            set(yaml, &["a", "b"], "2").unwrap(),
            "a:\n    b: 2\n    window: 10m\n    #b: 1\nb:\n    window: 1s # comment\n    c:\n        d: 1\n"
        );
        assert_eq!(
            set(yaml, &["b", "c", "d"], "3").unwrap(),
            "a:\n    window: 10m\n    #b: 1\nb:\n    window: 1s # comment\n    c:\n        d: 3\n"
        );
        assert_eq!(set(yaml, &["z", "d"], "3"), None);
    }

    /// Enough of an Ergo release's `default.yaml` for every setting to find its section.
    const DEFAULT: &str = "server:
    listeners:
        \"127.0.0.1:6667\":
        \"[::1]:6667\":
        \":6697\":
    unix-bind-mode: 0777
    max-line-len: 512
    ip-limits:
        count: false
accounts:
    nick-reservation:
        guest-nickname-format: \"Guest-*\"
channels:
    operator-only-creation: false
limits:
    multiline:
        max-bytes: 4096
        max-lines: 100
fakelag:
    window: 1s
    burst-limit: 5
    messages-per-window: 2
history:
    restrictions:
        expire-time: 1w
";

    fn held() -> Held {
        Held {
            retention: KEEP_EVERYTHING.to_owned(),
            line_length: 1024,
            multiline: Multiline {
                max_bytes: 16384,
                max_lines: 200,
            },
            rate: RateLimit {
                burst: 50,
                per_second: 20,
            },
        }
    }

    #[test]
    fn the_file_has_the_settings_as_they_re_held() {
        let settings = Settings {
            default_channels: vec!["#General".to_owned()],
            channel_creation: ChannelCreation::Operators,
            ..Settings::default()
        };
        let config: Value = serde_yaml::from_str(&file(DEFAULT, &settings, &held())).unwrap();
        assert_eq!(config["history"]["restrictions"]["expire-time"], 0);
        assert_eq!(config["server"]["max-line-len"], 1024);
        assert_eq!(config["limits"]["multiline"]["max-bytes"], 16384);
        assert_eq!(config["limits"]["multiline"]["max-lines"], 200);
        assert_eq!(config["fakelag"]["burst-limit"], 50);
        assert_eq!(config["fakelag"]["messages-per-window"], 20);
        assert_eq!(config["channels"]["operator-only-creation"], true);
        assert_eq!(config["channels"]["auto-join"][0], "#general");
        assert_eq!(
            config["server"]["ip-limits"]["max-concurrent-connections"],
            1024
        );
        assert_eq!(
            config["accounts"]["nick-reservation"]["guest-nickname-format"],
            "Unregistered-*"
        );
    }

    #[test]
    fn it_listens_as_the_image_does_whichever_file_it_starts_from() {
        let rendered = file(DEFAULT, &Settings::default(), &held());
        let config: Value = serde_yaml::from_str(&rendered).unwrap();
        let listeners = config["server"]["listeners"].as_mapping().unwrap();
        assert!(listeners.contains_key(":6667"));
        assert!(!listeners.contains_key("127.0.0.1:6667"));
        assert!(!listeners.contains_key("[::1]:6667"));
        // The image's own file, which listens so already, comes out the same.
        assert_eq!(file(&rendered, &Settings::default(), &held()), rendered);
        // YAML 1.1's octal, which a YAML 1.2 library would quote.
        assert!(rendered.contains("\n    unix-bind-mode: 0777\n"));
    }
}
