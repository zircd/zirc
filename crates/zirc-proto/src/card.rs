//! A card an integration posts, which every app draws the same way: CI's landings, alerts,
//! deploys, a team's own script (`docs/cards.md`). It's the `card` in a
//! [`crate::Event::Custom`]'s data, whatever the event's kind, and the event's text still
//! says it all for plain IRC clients and older apps, which ignore it. A later card with the
//! same id from the same nick replaces it, whole. Its buttons reach the integration that
//! posted it through zirc.dev, which decides nothing about them: the integration does.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::Event;

/// The key a card goes under in a custom event's data.
pub const KEY: &str = "card";

/// The longest a card is, as JSON: small enough that its event is one multiline message.
pub const MAX_BYTES: usize = 2000;

/// A card: see the module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields)]
pub struct Card {
    /// Unique within its sender: a later card with the same id from the same nick replaces
    /// it. 1–64 of letters, digits and `._:/-`, such as `monorepo/889`.
    pub id: String,
    /// One line, at most 80 characters.
    pub title: String,
    /// Its colour and icon; [`State::Info`] when it doesn't say.
    #[serde(default, skip_serializing_if = "is_default")]
    #[ts(as = "Option<State>", optional)]
    pub state: State,
    /// The agent session or nick it concerns, such as the session a landing is for. A
    /// [`Who::About`] button is for its owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    /// At most 8, shown as label and value.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<Field>>", optional)]
    pub fields: Vec<Field>,
    /// At most 4, shown under the fields.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<Link>>", optional)]
    pub links: Vec<Link>,
    /// At most 3 buttons.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<Action>>", optional)]
    pub actions: Vec<Action>,
    /// At most 10, in order: what it's doing and has done, such as a build's phases. Apps
    /// draw them under the fields, and fold them into one line once the card is done.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<Step>>", optional)]
    pub steps: Vec<Step>,
    /// One line under it all, such as who pressed what and how it went, or an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// A picture under its title, such as a preview deploy's screenshot: a file zirc.dev
    /// hosts, uploaded as a shared one is. Apps draw it only from zirc.dev, as they draw a
    /// shared file, never loading another address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<Image>,
}

/// A card's picture ([`Card::image`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "CardImage")]
pub struct Image {
    /// The file, as zirc.dev hosts it: an image.
    pub file: crate::File,
    /// What it shows, for a screen reader and where it isn't drawn: at most 120 characters.
    pub alt: String,
}

/// A card's colour and icon.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[ts(rename = "CardState")]
pub enum State {
    /// Nothing good or bad, as when a card doesn't say.
    #[default]
    Info,
    /// Waiting to start.
    Pending,
    /// Under way.
    Running,
    /// Done, and it went well.
    Passed,
    /// Done, and it didn't.
    Failed,
    /// Stopped before it was done.
    Cancelled,
    /// Needs a look.
    Warning,
}

/// A label and its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "CardField")]
pub struct Field {
    /// At most 24 characters.
    pub label: String,
    /// At most 120 characters, in zirc-chat's Markdown, so it can be a link.
    pub value: String,
}

/// A link under the fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "CardLink")]
pub struct Link {
    /// At most 24 characters.
    pub label: String,
    /// An `https://` address, at most 300 characters.
    pub url: String,
}

/// A step of what a card is about, such as a build's "Build and test".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields, rename = "CardStep")]
pub struct Step {
    /// At most 32 characters.
    pub name: String,
    /// How it stands: [`State::Pending`] until it starts, [`State::Cancelled`] when it was
    /// skipped; [`State::Info`] when it doesn't say.
    #[serde(default, skip_serializing_if = "is_default")]
    #[ts(as = "Option<State>", optional)]
    pub state: State,
    /// At most 32 characters beside it, such as `onto 43cdc6e` or `//web:e2e`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// How much of it is done, of [`Step::of`]: apps draw a bar and `81 / 214`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<u32>,
    /// How much there is to do, such as 214 targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<u32>,
    /// How many seconds it took, or has taken so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secs: Option<u32>,
}

/// A button. Pressing it reaches the integration that posted the card, which decides
/// whether the person may.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields, rename = "CardAction")]
pub struct Action {
    /// The integration's own name for it: 1–32 of lowercase letters, digits, `_` and `-`,
    /// unique in the card.
    pub id: String,
    /// At most 24 characters.
    pub label: String,
    /// Who apps show it to; [`Who::Members`] when it doesn't say.
    #[serde(default, skip_serializing_if = "is_default")]
    #[ts(as = "Option<Who>", optional)]
    pub who: Who,
    /// What apps ask before sending it, at most 120 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<String>,
}

/// Who may press a button, as apps show it: the integration decides.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[ts(rename = "CardWho")]
pub enum Who {
    /// The organisation's members: everyone in the channel, as when a button doesn't say.
    #[default]
    Members,
    /// Anyone in the channel: the same as [`Who::Members`], as only members are in its
    /// channels. Kept because cards say it.
    Anyone,
    /// The person the card is about: the owner of its `about` session, or that nick.
    About,
    /// The organisation's owners and admins.
    Admins,
    /// The organisation's owners.
    Owners,
}

impl Card {
    /// Why the card isn't one apps can draw, if it isn't: the first rule it breaks.
    ///
    /// # Errors
    ///
    /// A sentence saying which rule, such as `the title is longer than 80 characters`.
    pub fn validate(&self) -> Result<(), String> {
        let named = |text: &str, max: usize, extra: &str| {
            (1..=max).contains(&text.len())
                && text
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
        };
        if !named(&self.id, 64, "._:/-") {
            return Err("its id is 1-64 letters, digits and ._:/-".to_owned());
        }
        within("the title", &self.title, 80)?;
        if self.title.trim().is_empty() {
            return Err("it has no title".to_owned());
        }
        if let Some(about) = &self.about {
            within("about", about, 64)?;
        }
        if let Some(note) = &self.note {
            within("the note", note, 120)?;
        }
        at_most("fields", self.fields.len(), 8)?;
        at_most("links", self.links.len(), 4)?;
        at_most("actions", self.actions.len(), 3)?;
        at_most("steps", self.steps.len(), 10)?;
        for step in &self.steps {
            within("a step's name", &step.name, 32)?;
            if step.name.trim().is_empty() {
                return Err("a step has no name".to_owned());
            }
            if let Some(detail) = &step.detail {
                within("a step's detail", detail, 32)?;
            }
            if step
                .done
                .is_some_and(|done| step.of.is_none_or(|of| done > of))
            {
                return Err("a step has done more than its of".to_owned());
            }
        }
        for field in &self.fields {
            within("a field's label", &field.label, 24)?;
            within("a field's value", &field.value, 120)?;
        }
        for link in &self.links {
            within("a link's label", &link.label, 24)?;
            web("a link's url", &link.url)?;
        }
        for (i, action) in self.actions.iter().enumerate() {
            if !action.id.chars().all(|c| !c.is_ascii_uppercase()) || !named(&action.id, 32, "_-") {
                return Err("an action's id is 1-32 lowercase letters, digits, _ and -".to_owned());
            }
            if self.actions[..i].iter().any(|other| other.id == action.id) {
                return Err(format!("two actions are both {:?}", action.id));
            }
            within("an action's label", &action.label, 24)?;
            if let Some(confirm) = &action.confirm {
                within("an action's confirm", confirm, 120)?;
            }
        }
        if let Some(image) = &self.image {
            within("the image's alt", &image.alt, 120)?;
            if image.alt.trim().is_empty() {
                return Err("the image has no alt".to_owned());
            }
            web("the image's url", &image.file.url)?;
            if !image.file.mime.starts_with("image/") {
                return Err("the image isn't an image".to_owned());
            }
        }
        let bytes = serde_json::to_string(self).map_or(usize::MAX, |json| json.len());
        if bytes > MAX_BYTES {
            return Err(format!("it's {bytes} bytes as JSON, more than {MAX_BYTES}"));
        }
        Ok(())
    }

    /// A card called `title` with the id `id`, and nothing else yet: the builder every card
    /// starts from, CI's, a workflow's, an integration's adapter's and `zirc card`'s alike,
    /// so all of them are checked by [`Card::validate`] alone ([`Card::checked`]).
    #[must_use]
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            state: State::default(),
            about: None,
            fields: Vec::new(),
            links: Vec::new(),
            actions: Vec::new(),
            steps: Vec::new(),
            note: None,
            image: None,
        }
    }

    /// It in `state`.
    #[must_use]
    pub fn with_state(mut self, state: State) -> Self {
        self.state = state;
        self
    }

    /// It about `about`, an agent session or a nick.
    #[must_use]
    pub fn with_about(mut self, about: impl Into<String>) -> Self {
        self.about = Some(about.into());
        self
    }

    /// It with one more field.
    #[must_use]
    pub fn with_field(mut self, label: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.push(Field {
            label: label.into(),
            value: value.into(),
        });
        self
    }

    /// It with one more link.
    #[must_use]
    pub fn with_link(mut self, label: impl Into<String>, url: impl Into<String>) -> Self {
        self.links.push(Link {
            label: label.into(),
            url: url.into(),
        });
        self
    }

    /// It with one more button.
    #[must_use]
    pub fn with_action(mut self, action: Action) -> Self {
        self.actions.push(action);
        self
    }

    /// It with one more step.
    #[must_use]
    pub fn with_step(mut self, step: Step) -> Self {
        self.steps.push(step);
        self
    }

    /// It with `note` under it all.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// It with a picture under its title.
    #[must_use]
    pub fn with_image(mut self, image: Image) -> Self {
        self.image = Some(image);
        self
    }

    /// It, when apps can draw it ([`Card::validate`]).
    ///
    /// # Errors
    ///
    /// The first rule it breaks, as [`Card::validate`] says it.
    pub fn checked(self) -> Result<Self, String> {
        self.validate()?;
        Ok(self)
    }

    /// The card a custom event's `data` carries, if it carries one that apps can draw.
    #[must_use]
    pub fn of(data: &serde_json::Value) -> Option<Self> {
        let card: Self = serde_json::from_value(data.get(KEY)?.clone()).ok()?;
        card.validate().is_ok().then_some(card)
    }

    /// The card `event` carries, if it's a custom event with one.
    #[must_use]
    pub fn in_event(event: &Event) -> Option<Self> {
        match event {
            Event::Custom { data, .. } => Self::of(data),
            _ => None,
        }
    }

    /// A custom event's data carrying this card and nothing else.
    #[must_use]
    pub fn as_data(&self) -> serde_json::Value {
        self.into_data(serde_json::Value::Null)
    }

    /// `data` with this card in it, under [`KEY`]: data that isn't an object becomes one.
    ///
    /// # Panics
    ///
    /// Never: a card serializes to JSON.
    #[must_use]
    pub fn into_data(&self, data: serde_json::Value) -> serde_json::Value {
        let mut data = match data {
            serde_json::Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };
        data.insert(
            KEY.to_owned(),
            serde_json::to_value(self).expect("a card serializes"),
        );
        serde_json::Value::Object(data)
    }
}

impl std::str::FromStr for State {
    type Err = String;

    /// A state by its name, as JSON says it: `info`, `pending`, `running`, `passed`,
    /// `failed`, `cancelled` or `warning`.
    fn from_str(name: &str) -> Result<Self, String> {
        serde_json::from_value(serde_json::Value::String(name.to_owned())).map_err(|_| {
            format!(
                "{name:?} isn't a state: info, pending, running, passed, failed, cancelled or \
                 warning"
            )
        })
    }
}

impl std::str::FromStr for Who {
    type Err = String;

    /// Who by name, as JSON says it: `members`, `anyone`, `about`, `admins` or `owners`.
    fn from_str(name: &str) -> Result<Self, String> {
        serde_json::from_value(serde_json::Value::String(name.to_owned()))
            .map_err(|_| format!("{name:?} isn't who: members, anyone, about, admins or owners"))
    }
}

/// `spec`'s `<head>` and its `;key=value` options, as a command line or an adapter's config
/// writes a card's parts: `Build and test=running;done=81;of=214`.
fn options(spec: &str) -> (&str, Vec<(&str, &str)>) {
    let mut parts = spec.split(';');
    let head = parts.next().unwrap_or_default();
    let options = parts
        .filter_map(|part| part.split_once('='))
        .map(|(key, value)| (key.trim(), value.trim()))
        .collect();
    (head, options)
}

/// `head` split at its first `=`: what's before it, and what's after it if there's one.
fn named_value(head: &str) -> (&str, Option<&str>) {
    match head.split_once('=') {
        Some((name, value)) => (name.trim(), Some(value.trim())),
        None => (head.trim(), None),
    }
}

impl Field {
    /// A field written `<label>=<value>`.
    ///
    /// # Errors
    ///
    /// When there's no `=`.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let (label, value) = spec
            .split_once('=')
            .ok_or_else(|| format!("{spec:?} isn't <label>=<value>"))?;
        Ok(Self {
            label: label.trim().to_owned(),
            value: value.trim().to_owned(),
        })
    }
}

impl Link {
    /// A link written `<label>=<url>`.
    ///
    /// # Errors
    ///
    /// When there's no `=`.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let field = Field::parse(spec).map_err(|_| format!("{spec:?} isn't <label>=<url>"))?;
        Ok(Self {
            label: field.label,
            url: field.value,
        })
    }
}

impl Step {
    /// A step called `name`, its state not said.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            state: State::default(),
            detail: None,
            done: None,
            of: None,
            secs: None,
        }
    }

    /// A step written `<name>[=<state>][;detail=…][;done=…][;of=…][;secs=…]`: `Build and
    /// test=running;done=81;of=214`.
    ///
    /// # Errors
    ///
    /// An unknown state or option, or a count that isn't a number.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let (head, options) = options(spec);
        let (name, state) = named_value(head);
        let mut step = Self::new(name);
        if let Some(state) = state {
            step.state = state.parse()?;
        }
        let number = |key: &str, value: &str| {
            value
                .parse::<u32>()
                .map_err(|_| format!("a step's {key} is a number, not {value:?}"))
        };
        for (key, value) in options {
            match key {
                "detail" => step.detail = Some(value.to_owned()),
                "done" => step.done = Some(number(key, value)?),
                "of" => step.of = Some(number(key, value)?),
                "secs" => step.secs = Some(number(key, value)?),
                _ => return Err(format!("a step has no {key:?}: detail, done, of or secs")),
            }
        }
        Ok(step)
    }
}

impl Action {
    /// An action written `<id>=<label>[;who=…][;confirm=…]`: `retry=Retry;who=about`.
    ///
    /// # Errors
    ///
    /// No label, or an unknown who or option.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let (head, options) = options(spec);
        let (id, label) = named_value(head);
        let label = label.ok_or_else(|| format!("{spec:?} isn't <id>=<label>"))?;
        let mut action = Self {
            id: id.to_owned(),
            label: label.to_owned(),
            who: Who::default(),
            confirm: None,
        };
        for (key, value) in options {
            match key {
                "who" => action.who = value.parse()?,
                "confirm" => action.confirm = Some(value.to_owned()),
                _ => return Err(format!("an action has no {key:?}: who or confirm")),
            }
        }
        Ok(action)
    }
}

/// Where an app presses a card's button, in organisation `org`: zirc.dev relays it to the
/// integration that posts as the card's sender.
#[must_use]
pub fn press_path(org: &str) -> String {
    format!("{}/cards/actions", crate::irc::organisation_path(org))
}

/// A press of a card's button, as an app sends it to [`press_path`] as the person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "CardPress")]
pub struct Press {
    /// The channel the card is in, `#monorepo`.
    pub channel: String,
    /// The nick that posted the card.
    pub sender: String,
    /// The card's id.
    pub card: String,
    /// The button's id.
    pub action: String,
}

/// What zirc.dev answers a [`Press`] with, when the integration took it: what it said, if
/// anything. A refusal is an error status with `{error}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields, rename = "CardPressed")]
pub struct Pressed {
    /// What the integration said, such as "Retrying the failed step".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A press as zirc.dev relays it to the integration that posted the card: the JSON body of
/// its signed POST ([`SIGNATURE_HEADER`]). The integration decides whether [`By`] may do it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "CardRelayedPress")]
pub struct RelayedPress {
    /// This press's own id: an integration acts on each id once.
    pub id: String,
    /// The organisation's slug.
    pub org: String,
    /// The channel the card is in.
    pub channel: String,
    /// The card's id.
    pub card: String,
    /// The button's id.
    pub action: String,
    /// Who pressed it.
    pub by: By,
}

/// Who pressed a card's button, as zirc.dev vouches for them: an integration checks this
/// against the button's [`Who`] and the card's `about`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(rename = "CardPresser")]
pub struct By {
    /// Their handle in the organisation.
    pub handle: String,
    /// Their own nick there.
    pub nick: String,
    /// Their role there.
    pub role: crate::role::Role,
    /// Their machines' nicks there, so a card about one of their sessions is theirs.
    #[serde(default)]
    pub machines: Vec<String>,
}

/// The header zirc.dev signs a [`RelayedPress`] with:
/// `t=<unix seconds>,v1=<hex HMAC-SHA256 of "<t>.<body>" with the integration's secret>`.
/// zirc-core's `cards` signs and verifies one.
pub const SIGNATURE_HEADER: &str = "Zirc-Signature";

/// How old a signature may be, either way, in seconds: an integration refuses an older one.
pub const SIGNATURE_MAX_AGE_SECS: u64 = 300;

/// What a signature is the HMAC of: the time, a dot and the body, exactly as sent.
#[must_use]
pub fn signed_text(t: u64, body: &str) -> String {
    format!("{t}.{body}")
}

/// The [`SIGNATURE_HEADER`]'s value for a body signed at `t` with HMAC `mac`.
#[must_use]
pub fn signature_header(t: u64, mac: &[u8]) -> String {
    format!("t={t},v1={}", crate::hex(mac))
}

/// A [`SIGNATURE_HEADER`]'s parts: when it was signed, and the HMAC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    /// When, in unix seconds.
    pub t: u64,
    /// The HMAC-SHA256, as bytes.
    pub v1: Vec<u8>,
}

impl Signature {
    /// The parts of `header`.
    ///
    /// # Errors
    ///
    /// What's wrong with it, when it isn't `t=<seconds>,v1=<hex>`.
    pub fn parse(header: &str) -> Result<Self, &'static str> {
        let mut t = None;
        let mut v1 = None;
        for part in header.split(',') {
            match part.trim().split_once('=') {
                Some(("t", value)) => t = value.parse::<u64>().ok(),
                Some(("v1", value)) => v1 = hex(value),
                _ => {}
            }
        }
        match (t, v1) {
            (Some(t), Some(v1)) => Ok(Self { t, v1 }),
            _ => Err("a Zirc-Signature is t=<seconds>,v1=<hex>"),
        }
    }
}

/// `text`'s bytes, if it's an even number of hex digits.
fn hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || text.is_empty() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

/// A press signed as zirc.dev signs it, for every implementation of the signature to check
/// itself against: the secret, the time, the body and the header they make.
pub const SIGNATURE_EXAMPLE: [&str; 4] = [
    "zirc-example-secret",
    "1793750400",
    r##"{"id":"p1","org":"acme","channel":"#monorepo","card":"monorepo/889","action":"retry","by":{"handle":"ada","nick":"ada","role":"owner","machines":["ada-mbp"]}}"##,
    "t=1793750400,v1=a7edebe4c63e1fdd39d957c618a3835937ca6739d86237f8828e4e0930748426",
];

/// The card relay's types and constants as TypeScript, for the web app's generated file:
/// the press an app sends, what it's answered, the press relayed and who made it, and the
/// signature's header, age and example.
///
/// # Panics
///
/// Never: a string serializes to JSON.
#[must_use]
pub fn typescript(config: &ts_rs::Config) -> Vec<String> {
    let [secret, t, body, header] = SIGNATURE_EXAMPLE;
    vec![
        Press::decl(config),
        Pressed::decl(config),
        RelayedPress::decl(config),
        By::decl(config),
        crate::role::Role::decl(config),
        format!(
            "const ROLES: readonly Role[] = [{}];",
            crate::role::Role::ALL
                .map(|role| format!("{:?}", role.as_str()))
                .join(", ")
        ),
        format!("const CARD_SIGNATURE_HEADER = {SIGNATURE_HEADER:?};"),
        format!("const CARD_SIGNATURE_MAX_AGE_SECS = {SIGNATURE_MAX_AGE_SECS};"),
        format!(
            "const CARD_SIGNATURE_EXAMPLE = {{ secret: {secret:?}, t: {t}, body: {}, header: {header:?} }};",
            serde_json::to_string(body).expect("a string serializes")
        ),
    ]
}

/// Whether `value` is what a card that doesn't say has, so it's left out as written.
fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// Whether `text` is at most `max` characters, as `what` says when it isn't.
fn within(what: &str, text: &str, max: usize) -> Result<(), String> {
    if text.chars().count() > max {
        return Err(format!("{what} is longer than {max} characters"));
    }
    if text.contains(['\n', '\r']) {
        return Err(format!("{what} is more than one line"));
    }
    Ok(())
}

fn at_most(what: &str, count: usize, max: usize) -> Result<(), String> {
    if count > max {
        return Err(format!("it has {count} {what}, more than {max}"));
    }
    Ok(())
}

/// Whether `url` is an `https://` address of at most 300 characters.
fn web(what: &str, url: &str) -> Result<(), String> {
    within(what, url, 300)?;
    match url.strip_prefix("https://") {
        Some(rest) if !rest.is_empty() && !url.contains(char::is_whitespace) => Ok(()),
        _ => Err(format!("{what} isn't an https:// address")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn landing() -> Card {
        serde_json::from_value(serde_json::json!({
            "id": "monorepo/889",
            "title": "ci-target-links · build 889",
            "state": "failed",
            "about": "cc-97a6df",
            "fields": [
                { "label": "failed in", "value": "lint" },
                { "label": "target", "value": "[//web:e2e](https://zirc.dev/test-results/x)" }
            ],
            "links": [{ "label": "Open log", "url": "https://buildkite.com/zirc/monorepo/builds/889" }],
            "actions": [
                { "id": "retry", "label": "Retry failed step", "who": "about" },
                { "id": "cancel", "label": "Cancel", "who": "admins", "confirm": "Cancel build 889?" }
            ],
            "note": "retried by mattias"
        }))
        .unwrap()
    }

    #[test]
    fn reads_the_specs_example_from_a_custom_events_data() {
        let card = landing();
        assert_eq!(card.validate(), Ok(()));
        let event = Event::Custom {
            kind: crate::ci::LANDING_KIND.into(),
            data: card.into_data(serde_json::json!({"build": "889"})),
            text: "build 889 of ci-target-links failed in lint".into(),
        };
        assert_eq!(Card::in_event(&event), Some(card));
        let Event::Custom { data, .. } = &event else {
            unreachable!()
        };
        assert_eq!(data["build"], "889");
        // An event without one, or with one apps can't draw, has none.
        assert_eq!(Card::of(&serde_json::json!({"build": "889"})), None);
        assert_eq!(Card::of(&serde_json::json!({"card": {"id": "x"}})), None);
    }

    #[test]
    fn steps_are_named_short_and_count_up_to_what_there_is() {
        let step = |json: serde_json::Value| -> Step { serde_json::from_value(json).unwrap() };
        let mut card = landing();
        card.steps = vec![
            step(
                serde_json::json!({"name": "Rebase onto master", "state": "passed", "detail": "onto 43cdc6e", "secs": 15}),
            ),
            step(
                serde_json::json!({"name": "Build and test", "state": "running", "done": 81, "of": 214, "secs": 130}),
            ),
            step(serde_json::json!({"name": "Fast-forward master"})),
        ];
        assert_eq!(card.validate(), Ok(()));
        assert_eq!(card.steps[2].state, State::Info);
        card.steps[1].done = Some(215);
        assert_eq!(
            card.validate(),
            Err("a step has done more than its of".into())
        );
        card.steps[1].done = Some(81);
        card.steps[0].name = " ".into();
        assert_eq!(card.validate(), Err("a step has no name".into()));
        card.steps = vec![card.steps[1].clone(); 11];
        assert_eq!(card.validate(), Err("it has 11 steps, more than 10".into()));
    }

    #[test]
    fn a_card_or_button_that_doesnt_say_is_info_and_for_members() {
        let card: Card = serde_json::from_value(serde_json::json!({
            "id": "deploy/42",
            "title": "Deploy 1.4.2",
            "actions": [{ "id": "stop", "label": "Stop" }]
        }))
        .unwrap();
        assert_eq!(
            (card.state, card.actions[0].who),
            (State::Info, Who::Members)
        );
        assert_eq!(card.validate(), Ok(()));
        // And they're left out again, as written.
        let json = serde_json::to_value(&card).unwrap();
        assert!(json.get("state").is_none() && json["actions"][0].get("who").is_none());
        for (state, json) in [(State::Cancelled, "cancelled"), (State::Pending, "pending")] {
            assert_eq!(serde_json::to_value(state).unwrap(), json);
        }
        assert_eq!(serde_json::to_value(Who::Owners).unwrap(), "owners");
    }

    #[test]
    fn a_relayed_press_reads_as_zirc_dev_signs_it() {
        let [_, t, body, header] = SIGNATURE_EXAMPLE;
        let press: RelayedPress = serde_json::from_str(body).unwrap();
        assert_eq!(press.by.role, crate::role::Role::Owner);
        assert_eq!(press.by.machines, ["ada-mbp"]);
        let signature = Signature::parse(header).unwrap();
        assert_eq!(signature.t.to_string(), t);
        assert_eq!(signature_header(signature.t, &signature.v1), header);
        assert_eq!(signed_text(7, "{}"), "7.{}");
        for bad in ["", "t=1", "v1=ab", "t=x,v1=ab", "t=1,v1=abc", "t=1,v1=zz"] {
            assert!(Signature::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn refuses_what_apps_cant_draw_saying_why() {
        let refused = |change: fn(&mut Card)| {
            let mut card = landing();
            change(&mut card);
            card.validate().unwrap_err()
        };
        assert_eq!(
            refused(|card| card.id = "has space".into()),
            "its id is 1-64 letters, digits and ._:/-"
        );
        assert_eq!(
            refused(|card| card.title = "x".repeat(81)),
            "the title is longer than 80 characters"
        );
        assert_eq!(
            refused(|card| card.links[0].url = "http://buildkite.com".into()),
            "a link's url isn't an https:// address"
        );
        assert_eq!(
            refused(|card| card.actions[1].id = "retry".into()),
            "two actions are both \"retry\""
        );
        assert_eq!(
            refused(|card| card.actions[0].id = "Retry".into()),
            "an action's id is 1-32 lowercase letters, digits, _ and -"
        );
        assert_eq!(
            refused(|card| card.note = Some("one\ntwo".into())),
            "the note is more than one line"
        );
        assert_eq!(
            refused(|card| card.fields = vec![card.fields[0].clone(); 9]),
            "it has 9 fields, more than 8"
        );
    }

    #[test]
    fn builds_a_card_from_its_parts_as_a_command_line_writes_them() {
        let card = Card::new("deploy/42", "web · deploy 42")
            .with_state("running".parse().unwrap())
            .with_about("cc-5e20")
            .with_field("env", "prod")
            .with_link("Logs", "https://example.com/42")
            .with_action(
                Action::parse("rollback=Roll back;who=about;confirm=Roll back 42?").unwrap(),
            )
            .with_step(Step::parse("Canary=running;done=3;of=5;secs=40").unwrap())
            .with_note("rolling out")
            .checked()
            .unwrap();
        assert_eq!(card.fields, [Field::parse("env=prod").unwrap()]);
        assert_eq!(
            card.links,
            [Link::parse("Logs=https://example.com/42").unwrap()]
        );
        let action = &card.actions[0];
        assert_eq!(
            (action.who, action.confirm.as_deref()),
            (Who::About, Some("Roll back 42?"))
        );
        let step = &card.steps[0];
        assert_eq!(
            (step.state, step.done, step.of, step.secs),
            (State::Running, Some(3), Some(5), Some(40))
        );
    }

    #[test]
    fn says_what_is_wrong_with_a_part() {
        assert!(
            "busy"
                .parse::<State>()
                .unwrap_err()
                .contains("isn't a state")
        );
        assert!(
            Step::parse("Build;done=lots")
                .unwrap_err()
                .contains("a number")
        );
        assert!(
            Step::parse("Build;colour=red")
                .unwrap_err()
                .contains("no \"colour\"")
        );
        assert!(Action::parse("retry").unwrap_err().contains("<id>=<label>"));
        assert!(Field::parse("no value").is_err());
        // A card that breaks a rule isn't one.
        assert!(Card::new("", "untitled").checked().is_err());
    }

    #[test]
    fn an_image_is_a_hosted_image_with_words_for_it() {
        let file = |mime: &str| crate::File {
            url: "https://zirc.dev/m/acme/abc".into(),
            name: "preview.png".into(),
            mime: mime.into(),
            size: 2048,
            width: Some(1280),
            height: Some(800),
            duration_ms: None,
        };
        let card = |mime: &str, alt: &str| {
            Card::new("preview/7", "Preview of the blog").with_image(Image {
                file: file(mime),
                alt: alt.into(),
            })
        };
        assert!(card("image/png", "The blog's front page").checked().is_ok());
        assert!(
            card("image/png", " ")
                .checked()
                .unwrap_err()
                .contains("no alt")
        );
        assert!(
            card("text/plain", "words")
                .checked()
                .unwrap_err()
                .contains("isn't an image")
        );
    }

    #[test]
    fn the_largest_card_still_fits_one_message() {
        let long = |n: usize| "x".repeat(n);
        let card = |value: usize| Card {
            id: long(64),
            title: long(80),
            state: State::Running,
            about: Some(long(64)),
            fields: (0..8)
                .map(|_| Field {
                    label: long(24),
                    value: long(value),
                })
                .collect(),
            links: (0..4)
                .map(|_| Link {
                    label: long(24),
                    url: format!("https://{}", long(60)),
                })
                .collect(),
            actions: ["a", "b", "c"]
                .map(|id| Action {
                    id: id.into(),
                    label: long(24),
                    who: Who::Anyone,
                    confirm: Some(long(120)),
                })
                .into(),
            steps: Vec::new(),
            note: Some(long(120)),
            image: None,
        };
        // Every count and length at its limit is too much in all: the size is what binds.
        assert!(card(120).validate().unwrap_err().contains("bytes as JSON"));
        let largest = (0..=120)
            .rev()
            .map(card)
            .find(|card| card.validate().is_ok())
            .expect("a card that fits");
        let message = crate::Message::new(
            "zirc-alerts",
            Event::Custom {
                kind: crate::ci::LANDING_KIND.into(),
                data: largest
                    .into_data(serde_json::json!({"pipeline": "monorepo", "build": "99999"})),
                text: long(200),
            },
        );
        // One multiline message: Ergo's default multiline limit is 4096 bytes.
        let body = crate::encode_lines_up_to(&message, 4096).unwrap();
        assert!(body.len() <= 4096, "{}", body.len());
    }
}
