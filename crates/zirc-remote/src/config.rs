//! What a host may be asked to do: its config, and how a request is checked against it.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context as _, Result, bail, ensure};
use serde::Deserialize;

/// The commands every host answers by itself, which a config may not declare.
pub const RESERVED: [&str; 3] = ["help", "ping", "version"];

/// The longest a string argument is unless it says otherwise.
const MAX_LENGTH: usize = 500;

/// A host's config, as its JSON file says.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Accounts that may call every command, besides the owner and the owner's machines.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Integrations that may speak for the owner: a query from one of these accounts that
    /// begins `as <handle>` counts as that person's, when the handle is the owner's. The
    /// integration has checked who pressed, as the alert bridge does for a card's button.
    #[serde(default)]
    pub trust: Vec<String>,
    /// Channels the host sits in, saying nothing, so its owner's apps see it online and list
    /// it under Your machines, as zircd sits in its repositories' channels. It answers only
    /// what's said to it alone.
    #[serde(default)]
    pub channels: Vec<String>,
    /// The command that what isn't a command goes to, whole, as its one argument: a person
    /// can then just say what they want, `what's slow?`, which CI's `ask` answers.
    #[serde(default)]
    pub otherwise: Option<String>,
    /// What every command is held to.
    #[serde(default)]
    pub limits: Limits,
    /// The commands, by name.
    #[serde(default)]
    pub commands: BTreeMap<String, Command>,
}

impl Config {
    /// Reads a config and checks it.
    ///
    /// # Errors
    ///
    /// When the JSON isn't a config, or a command in it can't be used.
    pub fn parse(text: &str) -> Result<Self> {
        let config: Self = serde_json::from_str(text).context("the config isn't valid")?;
        for account in &config.trust {
            ensure!(
                !account.is_empty() && !account.contains(char::is_whitespace),
                "`trust` holds account names"
            );
        }
        for channel in &config.channels {
            ensure!(
                channel.starts_with('#')
                    && !channel.contains(|c: char| c.is_whitespace() || c == ','),
                "`channels` holds channels, `#name`: not {channel:?}"
            );
        }
        if let Some(name) = &config.otherwise {
            ensure!(
                config.commands.contains_key(name),
                "`otherwise` names `{name}`, which isn't one of the commands"
            );
        }
        config.limits.timeout().context("limits.timeout")?;
        for (name, command) in &config.commands {
            command
                .check(name)
                .with_context(|| format!("command `{name}`"))?;
        }
        Ok(config)
    }
}

/// What every command is held to.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Limits {
    /// How long a command may run unless it says otherwise: `30s`, `2m` or `500ms`.
    #[serde(default = "default_timeout")]
    pub timeout: String,
    /// The most output a reply carries, in bytes.
    #[serde(default = "default_output_bytes")]
    pub output_bytes: usize,
    /// The environment variables a command sees, besides its own arguments.
    #[serde(default)]
    pub env: Vec<String>,
}

fn default_timeout() -> String {
    "30s".to_owned()
}

fn default_output_bytes() -> usize {
    3500
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: default_timeout(),
            output_bytes: default_output_bytes(),
            env: Vec::new(),
        }
    }
}

impl Limits {
    /// How long a command may run unless it says otherwise.
    ///
    /// # Errors
    ///
    /// When it isn't a duration such as `30s`, `2m` or `500ms`.
    pub fn timeout(&self) -> Result<Duration> {
        duration(&self.timeout)
    }
}

fn duration(text: &str) -> Result<Duration> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: u64 = number
        .parse()
        .with_context(|| format!("{text:?} isn't a duration like 30s, 2m or 500ms"))?;
    match unit {
        "ms" => Ok(Duration::from_millis(number)),
        "s" => Ok(Duration::from_secs(number)),
        "m" => Ok(Duration::from_secs(number.saturating_mul(60))),
        _ => bail!("{text:?} isn't a duration like 30s, 2m or 500ms"),
    }
}

/// Who may call a command.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Who {
    /// The owner's own account, which is where their apps connect, and the accounts in
    /// `allow`. Not an agent: agents connect as the owner's machines.
    #[default]
    Owner,
    /// The owner, and every machine of theirs, where their agents run.
    Mine,
}

/// One command the host serves.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    /// One line shown by `help`.
    pub summary: String,
    /// Who may call it.
    #[serde(default)]
    pub who: Who,
    /// Its arguments, by name.
    #[serde(default)]
    pub args: BTreeMap<String, Arg>,
    /// The program and its arguments. `{name}` is replaced by an argument's value inside
    /// one element. No shell runs.
    pub exec: Vec<String>,
    /// How long it may run, instead of the limit.
    #[serde(default)]
    pub timeout: Option<String>,
    /// Whether calls may overlap. Unless it says so, a call while another runs is told it's
    /// busy, so a steer is never run twice at once.
    #[serde(default)]
    pub parallel: bool,
    /// The secrets it gets, as environment variables: the variable's name, then the name of
    /// the secret the host was started with. Only this command's process has one, and only
    /// while it runs; a command that doesn't name a secret never sees it.
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
}

impl Command {
    fn check(&self, name: &str) -> Result<()> {
        ensure!(
            name.starts_with(|c: char| c.is_ascii_lowercase())
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "a command's name is lowercase letters, digits and hyphens, starting with a letter"
        );
        ensure!(
            !RESERVED.contains(&name),
            "`{name}` is a command every host has"
        );
        ensure!(!self.summary.trim().is_empty(), "it needs a summary");
        ensure!(!self.exec.is_empty(), "it needs an `exec`");
        for (variable, secret) in &self.secrets {
            ensure!(
                variable.starts_with(|c: char| c.is_ascii_uppercase() || c == '_')
                    && variable
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                    && !secret.is_empty(),
                "`secrets` maps an environment variable, such as ANTHROPIC_API_KEY, to a secret's name"
            );
        }
        for part in &self.exec {
            for placeholder in placeholders(part) {
                ensure!(
                    self.args.contains_key(placeholder),
                    "`exec` uses {{{placeholder}}}, which isn't a declared argument"
                );
            }
        }
        if let Some(timeout) = &self.timeout {
            duration(timeout).context("timeout")?;
        }
        Ok(())
    }

    /// How long it may run, given the host's limit.
    ///
    /// # Errors
    ///
    /// When its timeout isn't a duration; [`Config::parse`] has checked.
    pub fn timeout(&self, limits: &Limits) -> Result<Duration> {
        self.timeout
            .as_deref()
            .map_or_else(|| limits.timeout(), duration)
    }

    /// The values for its arguments from a request's `key=value` words and its bare text:
    /// the bare text is the value of the one argument left, when there is exactly one.
    ///
    /// # Errors
    ///
    /// Why the request doesn't fit the command, in words for whoever asked.
    pub fn bind(
        &self,
        named: Vec<(String, String)>,
        rest: &str,
    ) -> Result<BTreeMap<String, String>, String> {
        let mut values = BTreeMap::new();
        for (key, value) in named {
            if !self.args.contains_key(&key) {
                return Err(format!("it has no argument `{key}`"));
            }
            values.insert(key, value);
        }
        if !rest.is_empty() {
            let free: Vec<&String> = self
                .args
                .keys()
                .filter(|key| !values.contains_key(*key))
                .collect();
            match free.as_slice() {
                [key] => {
                    values.insert((*key).clone(), rest.to_owned());
                }
                _ => return Err(format!("name its arguments as key=value, not `{rest}`")),
            }
        }
        for (key, arg) in &self.args {
            if values.contains_key(key) {
                continue;
            }
            match &arg.default {
                Some(default) => {
                    values.insert(key.clone(), default.clone());
                }
                None if arg.is_required() => return Err(format!("it needs `{key}`")),
                None => {}
            }
        }
        for (key, value) in &values {
            if let Some(arg) = self.args.get(key) {
                arg.check(value).map_err(|why| format!("`{key}` {why}"))?;
            }
        }
        Ok(values)
    }

    /// The program and its arguments for a request's values.
    #[must_use]
    pub fn argv(&self, values: &BTreeMap<String, String>) -> Vec<String> {
        self.exec.iter().map(|part| fill(part, values)).collect()
    }

    /// One line for `help`: its name, summary and arguments.
    #[must_use]
    pub fn describe(&self, name: &str) -> String {
        let args: Vec<String> = self
            .args
            .iter()
            .map(|(key, arg)| {
                if arg.is_required() {
                    key.clone()
                } else {
                    format!("{key}?")
                }
            })
            .collect();
        if args.is_empty() {
            format!("{name}: {}", self.summary)
        } else {
            format!("{name} {}: {}", args.join(" "), self.summary)
        }
    }
}

/// What an argument may be.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Any text without control characters, up to its `maxLength`.
    #[default]
    String,
    /// A whole number, within its `min` and `max`.
    Integer,
    /// `true` or `false`.
    Boolean,
}

/// One argument of a command.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Arg {
    /// What it is.
    #[serde(rename = "type", default)]
    pub kind: Kind,
    /// The only values it takes, when there are some.
    #[serde(rename = "enum", default)]
    pub choices: Vec<String>,
    /// The least an integer may be.
    pub min: Option<i64>,
    /// The most an integer may be.
    pub max: Option<i64>,
    /// The most characters a string may have.
    pub max_length: Option<usize>,
    /// The value when a request gives none.
    pub default: Option<String>,
    /// Whether a request must give it: so unless it has a default.
    pub required: Option<bool>,
    /// What `help` says it is for.
    #[serde(default)]
    pub description: String,
}

impl Arg {
    /// Whether a request must give it.
    #[must_use]
    pub fn is_required(&self) -> bool {
        self.required.unwrap_or(self.default.is_none())
    }

    /// Whether `value` is one it takes.
    ///
    /// # Errors
    ///
    /// Why not, as the end of a sentence about the argument.
    pub fn check(&self, value: &str) -> Result<(), String> {
        if !self.choices.is_empty() && !self.choices.iter().any(|choice| choice == value) {
            return Err(format!("is one of {}", self.choices.join(", ")));
        }
        match self.kind {
            Kind::String => {
                if value.chars().any(char::is_control) {
                    return Err("can't hold control characters".to_owned());
                }
                let most = self.max_length.unwrap_or(MAX_LENGTH);
                if value.chars().count() > most {
                    return Err(format!("is at most {most} characters"));
                }
            }
            Kind::Integer => {
                let number: i64 = value.parse().map_err(|_| "is a whole number".to_owned())?;
                if self.min.is_some_and(|min| number < min) {
                    return Err(format!("is at least {}", self.min.unwrap_or_default()));
                }
                if self.max.is_some_and(|max| number > max) {
                    return Err(format!("is at most {}", self.max.unwrap_or_default()));
                }
            }
            Kind::Boolean => {
                if value != "true" && value != "false" {
                    return Err("is true or false".to_owned());
                }
            }
        }
        Ok(())
    }
}

/// The names between braces in `text`.
fn placeholders(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else { break };
        found.push(&after[..end]);
        rest = &after[end + 1..];
    }
    found
}

/// `part` with each `{name}` replaced by its value, in one pass: a value is never searched
/// for placeholders, so one argument can't reach another.
fn fill(part: &str, values: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = part;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find('}') {
            out.push_str(values.get(&after[..end]).map_or("", String::as_str));
            rest = &after[end + 1..];
        } else {
            out.push_str(&rest[start..]);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

/// A request's text, read: a command, its `key=value` words, and its other words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The first word.
    pub command: String,
    /// The words that are `key=value`.
    pub named: Vec<(String, String)>,
    /// The other words, joined by spaces.
    pub rest: String,
}

/// A request's text read as a [`Request`]: words split on spaces, and a double-quoted run
/// is one word. `None` when it has no words.
#[must_use]
pub fn request(text: &str) -> Option<Request> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut started = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            c => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    let mut words = words.into_iter();
    let command = words.next()?;
    let mut named = Vec::new();
    let mut rest = Vec::new();
    for word in words {
        match word.split_once('=') {
            Some((key, value))
                if !key.is_empty()
                    && key
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') =>
            {
                named.push((key.to_owned(), value.to_owned()));
            }
            _ => rest.push(word),
        }
    }
    Some(Request {
        command,
        named,
        rest: rest.join(" "),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"{
        "commands": {
            "log": {
                "summary": "Tail of the log",
                "who": "mine",
                "args": { "lines": { "type": "integer", "min": 1, "max": 200, "default": "50" } },
                "exec": ["tail", "-n", "{lines}", "/run/ci/build.log"]
            },
            "ask": {
                "summary": "Ask a question",
                "args": { "question": { "maxLength": 20 } },
                "exec": ["answer", "{question}"]
            }
        }
    }"#;

    #[test]
    fn reads_a_config_and_binds_a_request_to_a_command() {
        let config = Config::parse(CONFIG).unwrap();
        let log = &config.commands["log"];
        let asked = request("log lines=7").unwrap();
        assert_eq!(asked.command, "log");
        let values = log.bind(asked.named, &asked.rest).unwrap();
        assert_eq!(log.argv(&values), ["tail", "-n", "7", "/run/ci/build.log"]);
        let values = log.bind(Vec::new(), "").unwrap();
        assert_eq!(log.argv(&values)[2], "50", "its default");
    }

    #[test]
    fn bare_words_are_the_one_argument_left() {
        let config = Config::parse(CONFIG).unwrap();
        let asked = request("ask why did it fail").unwrap();
        let values = config.commands["ask"]
            .bind(asked.named, &asked.rest)
            .unwrap();
        assert_eq!(values["question"], "why did it fail");
    }

    #[test]
    fn refuses_what_does_not_fit() {
        let config = Config::parse(CONFIG).unwrap();
        let log = &config.commands["log"];
        assert!(log.bind(vec![("lines".into(), "0".into())], "").is_err());
        assert!(log.bind(vec![("lines".into(), "many".into())], "").is_err());
        assert!(log.bind(vec![("other".into(), "1".into())], "").is_err());
        let ask = &config.commands["ask"];
        assert!(ask.bind(Vec::new(), &"x".repeat(21)).is_err());
        assert!(ask.bind(Vec::new(), "").is_err(), "it is required");
    }

    #[test]
    fn a_value_never_reaches_another_argument() {
        let mut values = BTreeMap::new();
        values.insert("a".to_owned(), "{b}".to_owned());
        values.insert("b".to_owned(), "secret".to_owned());
        assert_eq!(fill("{a}", &values), "{b}");
    }

    #[test]
    fn refuses_a_config_it_cannot_use() {
        for bad in [
            r#"{"trust": [""]}"#,
            r#"{"commands": {"help": {"summary": "x", "exec": ["x"]}}}"#,
            r#"{"commands": {"a": {"summary": "x", "exec": ["x", "{nope}"]}}}"#,
            r#"{"commands": {"a": {"summary": "x", "exec": []}}}"#,
            r#"{"limits": {"timeout": "soon"}}"#,
            r#"{"unknown": 1}"#,
            r#"{"otherwise": "missing"}"#,
            r#"{"channels": ["no-hash"]}"#,
        ] {
            assert!(Config::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_command_names_the_secrets_it_gets() {
        let config = Config::parse(
            r#"{"commands": {"ask": {"summary": "x", "exec": ["x"],
                "secrets": {"ANTHROPIC_API_KEY": "anthropic"}}}}"#,
        )
        .unwrap();
        assert_eq!(
            config.commands["ask"].secrets["ANTHROPIC_API_KEY"],
            "anthropic"
        );
        for bad in [
            r#"{"commands": {"a": {"summary": "x", "exec": ["x"], "secrets": {"lower": "s"}}}}"#,
            r#"{"commands": {"a": {"summary": "x", "exec": ["x"], "secrets": {"KEY": ""}}}}"#,
        ] {
            assert!(Config::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn quotes_make_one_word() {
        let asked = request(r#"say text="two words" more"#).unwrap();
        assert_eq!(asked.command, "say");
        assert_eq!(asked.named, [("text".to_owned(), "two words".to_owned())]);
        assert_eq!(asked.rest, "more");
        assert!(request("   ").is_none());
    }
}
