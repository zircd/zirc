//! Who a row is from, as every app heads it: an avatar, a name line, and what a screen
//! reader says. A person shows as themselves; an agent session by its name, with its
//! owner's after it and a robot on the owner's photo; one of zirc's apps by its label.

use super::conversation::{Entry, EntryKind};
use super::{Chat, SenderKind, ZIRC_LABEL, app_label, nick_degrees};

/// What a row's avatar shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Avatar {
    /// A person: their photo, or their initials.
    Person {
        /// Their nick, to find their photo and initials by.
        nick: String,
    },
    /// An agent: its owner's photo with a robot badge.
    Agent {
        /// The tool the agent runs in, such as `claude-code` or `codex`
        /// ([`zirc_proto::CLAUDE_CODE`]), for its badge; `None` for a generic one.
        tool: Option<String>,
        /// Its colour, a hue in degrees from 0 up to 359, the same every time for the same
        /// session ([`super::nick_degrees`]).
        hue: u16,
        /// Whose agent it is, by nick, for the photo.
        owner: String,
        /// The session's id, `cc-598aa1`; `None` for the machine itself.
        session: Option<String>,
        /// The picture it chose from the gallery, one of [`zirc_proto::AGENT_AVATARS`]'
        /// ids, such as `rocket`; `None` when it chose none, or one the gallery hasn't, so
        /// apps show its tool's icon instead.
        picture: Option<String>,
    },
    /// One of zirc's apps, CI or Alerts: a glyph rather than a photo.
    App {
        /// What it draws, at most two letters: `CI`, `AL`.
        glyph: String,
        /// Its label, `ci`.
        label: String,
    },
}

/// What tags one of zirc's apps after its name, CI or Alerts, in every app's rows and lists:
/// `Alerts APP`. An app never has a presence dot.
pub const APP_TAG: &str = "APP";

/// What tags a role's lines after its name, in a room as in our conversation with it (mock
/// `MacTools`): `oncall ROLE`.
pub const ROLE_TAG: &str = "ROLE";

/// What tags an agent session's own conversation in its header, after its title:
/// `checkout · Maya Chen AGENT`.
pub const AGENT_TAG: &str = "AGENT";

/// Who a row is from, as its head shows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Speaker {
    /// The avatar.
    pub avatar: Avatar,
    /// The name line: a person's name, an agent session's name, an app's label.
    pub name: String,
    /// What follows the name, muted: `mattias's agent` for an agent session.
    pub detail: Option<String>,
    /// What a screen reader says of them: `zirc-chat, agent of mattias`.
    pub label: String,
    /// Whether it's one of zirc's apps, named by the app's name and tagged [`APP_TAG`].
    pub is_app: bool,
}

/// The avatar of the agent `entry` is from, when it's from one: what [`Chat::speaker`] gives
/// it, without needing a [`Chat`].
#[must_use]
pub fn agent_avatar(entry: &Entry) -> Option<Avatar> {
    let nick = entry.from.as_str();
    let owner = entry.owner.as_deref()?;
    if super::sender_kind(nick, Some(owner)) != SenderKind::Agent {
        return None;
    }
    let message = match &entry.kind {
        EntryKind::Zirc(message) => Some(message),
        _ => None,
    };
    let session = entry.agent_session().map(str::to_owned);
    Some(Avatar::Agent {
        tool: message.and_then(|message| message.tool.clone()),
        picture: message
            .and_then(|message| message.avatar.as_deref())
            .and_then(zirc_proto::agent_avatar)
            .map(str::to_owned),
        hue: nick_degrees(session.as_deref().unwrap_or(nick)),
        owner: owner.to_owned(),
        session,
    })
}

/// What [`Chat::owner_name`] calls us, the person the app is for: `your agent`.
pub const YOU: &str = "you";

/// Whose something is, from its owner's name ([`Chat::owner_name`]): `your` for us, else
/// `Ada's`. Every `… agent` and `… agents` says it this way.
#[must_use]
pub fn possessive(owner: &str) -> String {
    if owner == YOU {
        "your".to_owned()
    } else {
        format!("{owner}'s")
    }
}

/// A machine as people read it after its owner, the one place it's composed: `Henrik's mbp`,
/// `your mbp`. `owner` is the owner's nick, `name` how they show ([`Chat::owner_name`]), and
/// `machine` the machine's nick, whose `<owner>-` the owner's name already says.
#[must_use]
pub fn machine_words(owner: &str, name: &str, machine: &str) -> String {
    let short = machine
        .strip_prefix(owner)
        .and_then(|rest| rest.strip_prefix('-'))
        .filter(|rest| !rest.is_empty())
        .unwrap_or(machine);
    format!("{} {short}", possessive(name))
}

/// An agent's name line, the one place it's composed, agent first (the user's choice): an
/// agent session as its name, `analytics`, with `mattias's agent` after it, muted; the
/// machine itself, with no session, as `mattias's agent` alone. `owner` is as the owner
/// shows: their profile's name, or else their nick.
#[must_use]
pub fn agent_name(session: Option<&str>, owner: &str) -> (String, Option<String>) {
    let agent = format!("{} agent", possessive(owner));
    match session {
        Some(session) => (session.to_owned(), Some(agent)),
        None => (agent, None),
    }
}

/// What a screen reader says of an agent, as [`agent_name`] shows it: `analytics, agent of
/// mattias`, or `mattias's agent` for the machine itself.
#[must_use]
pub fn agent_label(session: Option<&str>, owner: &str) -> String {
    match session {
        Some(session) if owner == YOU => format!("{session}, your agent"),
        Some(session) => format!("{session}, agent of {owner}"),
        None => format!("{} agent", possessive(owner)),
    }
}

/// Who something was from, as plain text names them: by `nick`, whose machine it is
/// (`owner`, as its host said), and, for a zirc message, its `message`. An agent session by the
/// name `message` gives, or else as `session_name` says ([`Chat::session_label`], or its id with
/// no [`Chat`] to ask), the machine itself (or a plain line from it) by its name, the owner as
/// `owner_name` says ([`Chat::owner_name`], or their nick with no [`Chat`] to ask), an app by
/// its label, and a person as `owner_name` says too, as they show.
#[must_use]
pub fn sender_text(
    nick: &str,
    owner: Option<&str>,
    message: Option<&zirc_proto::Message>,
    owner_name: &dyn Fn(&str) -> String,
    session_name: &dyn Fn(&str) -> String,
) -> String {
    match super::sender_kind(nick, owner) {
        SenderKind::Integration => {
            app_name(&app_label(nick, message).unwrap_or_else(|| ZIRC_LABEL.to_owned()))
        }
        SenderKind::Agent => {
            let owner = owner_name(owner.unwrap_or(nick));
            let session = message
                .and_then(|message| agent_session(nick, message))
                .map(str::to_owned)
                .map(|session| {
                    message
                        .and_then(|message| message.name.as_deref())
                        .and_then(zirc_proto::session_name)
                        .unwrap_or_else(|| session_name(&session))
                });
            match session {
                Some(_) => agent_name(session.as_deref(), &owner).0,
                None => nick.to_owned(),
            }
        }
        SenderKind::Person => owner_name(nick),
    }
}

/// The mark for an agent's tool, as apps draw it on the agent's colour when its session
/// chose no picture: ✳ for Claude Code, ◎ for Codex, ⚙ for any other.
#[must_use]
pub fn tool_mark(tool: Option<&str>) -> &'static str {
    match tool {
        Some(zirc_proto::CLAUDE_CODE) => "✳",
        Some(zirc_proto::CODEX) => "◎",
        _ => "⚙",
    }
}

/// The agent session `message`, sent as `nick`, came from: its session's id, `cc-598aa1`.
/// None from the machine itself, or from one of zirc's apps, whatever session it names.
#[must_use]
pub fn agent_session<'a>(nick: &str, message: &'a zirc_proto::Message) -> Option<&'a str> {
    let session = message.session.as_str();
    (!session.is_empty()
        && session != zirc_proto::NO_SESSION
        && !session.eq_ignore_ascii_case(nick)
        && super::sender_kind(nick, None) != SenderKind::Integration)
        .then_some(session)
}

impl Chat {
    /// The machine `nick` as people read it ([`machine_words`]): `Henrik's mbp`, `your mbp`;
    /// its nick where nobody's machine it is.
    pub(super) fn machine_said(&self, nick: &str) -> String {
        match self.whose(nick) {
            Some((owner, machine)) => machine_words(&owner, &self.owner_name(&owner), &machine),
            None => nick.to_owned(),
        }
    }
}

impl Chat {
    /// What a message's head calls whoever said something, [`Speaker::name`], by their
    /// `nick` and the agent `session` it came from, for where there's no message to ask,
    /// such as a search's match from the organisation's history: an agent session by its
    /// name, an app by its and a person as they show. Never a machine's nick.
    #[must_use]
    pub fn sender_name(&self, nick: &str, session: Option<&str>) -> String {
        match self.sender_kind(nick) {
            SenderKind::Integration => {
                app_name(&app_label(nick, None).unwrap_or_else(|| ZIRC_LABEL.to_owned()))
            }
            SenderKind::Agent => {
                let Some(session) = session else {
                    return nick.to_owned();
                };
                agent_name(
                    Some(&self.session_label(session)),
                    &self.owner_name(self.owner_of(nick)),
                )
                .0
            }
            SenderKind::Person => self.shown_name(nick),
        }
    }

    /// What a row's avatar shows for whoever said something, by their `nick`, where there's
    /// no message to ask, as [`Chat::sender_name`] names them: an app's glyph in its square,
    /// an agent session's owner, a person themselves.
    #[must_use]
    pub fn sender_avatar(&self, nick: &str) -> Avatar {
        match self.sender_kind(nick) {
            SenderKind::Integration => {
                let label = app_label(nick, None).unwrap_or_else(|| ZIRC_LABEL.to_owned());
                Avatar::App {
                    glyph: app_glyph(&label),
                    label,
                }
            }
            SenderKind::Agent => Avatar::Person {
                nick: self.owner_of(nick).to_owned(),
            },
            SenderKind::Person => Avatar::Person {
                nick: nick.to_owned(),
            },
        }
    }

    /// Whose `nick` is: its owner's handle when it's a machine, as its host says
    /// ([`Chat::machine_owner`]); anyone else's nick is their own.
    #[must_use]
    pub fn owner_of<'a>(&'a self, nick: &'a str) -> &'a str {
        self.machine_owner(nick).unwrap_or(nick)
    }

    /// What to call `owner`, a person by nick, where something names them as an agent's
    /// owner (`Ada's agent`): as they show ([`Chat::shown_name`]), and us as [`YOU`], so our
    /// own read `your agent`. Every owner's name is this one.
    #[must_use]
    pub fn owner_name(&self, owner: &str) -> String {
        if owner.eq_ignore_ascii_case(self.me()) {
            YOU.to_owned()
        } else {
            self.shown_name(owner)
        }
    }
}

/// An app's name, as everything that shows it says it, from its `label`: a short one as the
/// letters it stands for, `CI`; a longer one as a word, `Alerts`, as all capitals read as
/// a tag; zirc's own, [`ZIRC_LABEL`], as zirc's name, [`zirc_proto::NAME`].
#[must_use]
pub fn app_name(label: &str) -> String {
    if label == ZIRC_LABEL {
        return zirc_proto::NAME.to_owned();
    }
    if label.chars().count() <= 3 {
        return label.to_uppercase();
    }
    let mut chars = label.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// What an app with `label` draws in its square: the label's first two letters, `CI` or
/// `AL`, as a whole label such as `ALERTS` can't be read at an avatar's size; zirc's own,
/// [`ZIRC_LABEL`], its initial alone, `Z`.
pub(super) fn app_glyph(label: &str) -> String {
    if label == ZIRC_LABEL {
        return zirc_proto::NAME.chars().take(1).collect();
    }
    label.chars().take(2).collect::<String>().to_uppercase()
}

impl Chat {
    /// Who `entry` is from, as a row's head shows them.
    #[must_use]
    pub fn speaker(&self, entry: &Entry) -> Speaker {
        let nick = entry.from.as_str();
        let message = match &entry.kind {
            EntryKind::Zirc(message) => Some(message),
            _ => None,
        };
        // A machine as its host said when it spoke, though it's gone now.
        let kind = match (self.sender_kind(nick), &entry.owner) {
            (SenderKind::Person, Some(_)) => SenderKind::Agent,
            (kind, _) => kind,
        };
        match kind {
            SenderKind::Integration => {
                let label = app_label(nick, message).unwrap_or_else(|| ZIRC_LABEL.to_owned());
                Speaker {
                    avatar: Avatar::App {
                        glyph: app_glyph(&label),
                        label: label.clone(),
                    },
                    name: app_name(&label),
                    detail: None,
                    label: format!("{}, app", app_name(&label)),
                    is_app: true,
                }
            }
            SenderKind::Agent => {
                let owner = entry
                    .owner
                    .clone()
                    .unwrap_or_else(|| self.owner_of(nick).to_owned());
                let session = entry.agent_session().map(str::to_owned);
                let session_name = session.as_deref().map(|session| {
                    message
                        .and_then(|message| message.name.as_deref())
                        .and_then(zirc_proto::session_name)
                        .unwrap_or_else(|| self.session_label(session))
                });
                let shown = self.owner_name(&owner);
                if session_name.is_none() {
                    // The machine itself, not one of its agents: its own name, with the
                    // agents' glyph, never "your agent".
                    let name = nick.to_owned();
                    return Speaker {
                        avatar: Self::agents_avatar(),
                        label: format!("{name}, {} machine", possessive(&shown)),
                        detail: Some(format!("{} machine", possessive(&shown))),
                        name,
                        is_app: false,
                    };
                }
                let (name, detail) = agent_name(session_name.as_deref(), &shown);
                let label = agent_label(session_name.as_deref(), &shown);
                Speaker {
                    avatar: agent_avatar(entry).unwrap_or(Avatar::Person { nick: owner }),
                    name,
                    detail,
                    label,
                    is_app: false,
                }
            }
            SenderKind::Person => {
                let name = self.shown_name(nick);
                Speaker {
                    avatar: Avatar::Person {
                        nick: nick.to_owned(),
                    },
                    label: name.clone(),
                    name,
                    detail: None,
                    is_app: false,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_is_its_owners_by_its_own_name() {
        assert_eq!(
            machine_words("henrik", "Henrik", "henrik-mbp"),
            "Henrik's mbp"
        );
        assert_eq!(machine_words("henrik", YOU, "henrik-mbp"), "your mbp");
        // A nick that isn't the owner's and a name is said whole.
        assert_eq!(machine_words("henrik", "Henrik", "ci-1"), "Henrik's ci-1");
        assert_eq!(
            machine_words("henrik", "Henrik", "henrik-"),
            "Henrik's henrik-"
        );
    }
}
