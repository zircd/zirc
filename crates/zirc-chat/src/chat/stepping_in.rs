//! Stepping in: the menu a working row of the Office opens, with its ⋯ (the user's choices,
//! 28 Nov 2026: zirc leans on watching what agents do and stepping in). A coworker at its
//! desk or a session at a hot desk offers Message, Pause (Resume while paused) and Open the
//! conversation. Only the session's own person pauses it, as its machine's zircd takes a
//! pause from no one else ([`zirc_proto::pause`]); a session with no watch open can't hear
//! one, so Pause is offered greyed, saying so. Every word and rule is here; apps only draw.

use serde::Serialize;
use ts_rs::TS;
use zirc_proto::pause::Pause;
use zirc_proto::{Level, Message};

use super::face::Face;
use super::office::DeskState;
use super::{Chat, Line, Listening, agent_channel, conversation, fold};

/// What a row's ⋯ says to a screen reader: `Step in with John`.
#[must_use]
pub fn menu_label(name: &str) -> String {
    format!("Step in with {name}")
}

/// Under Message: when the message reaches it.
pub const MESSAGE_UNDER: &str = "It hears it on its watch, between steps";

/// The menu's Pause.
pub const PAUSE: &str = "Pause";

/// Under Pause.
pub const PAUSE_UNDER: &str = "Stops at a safe point and waits for you";

/// Under Pause, greyed, while no watch of the session's is open.
pub const NOT_WATCHING: &str = "Not watching zirc now, so it can't hear a pause";

/// The menu's Resume, in place of Pause while the session is paused.
pub const RESUME: &str = "Resume";

/// Under Resume.
pub const RESUME_UNDER: &str = "Carries on where it stopped";

/// The menu's Open.
pub const OPEN: &str = "Open the conversation";

/// Under Open.
pub const OPEN_UNDER: &str = "What it's said and done today";

/// What an item of a [`DeskMenu`] does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum DeskAction {
    /// Opens [`DeskMenuItem::opens`] to write to it.
    Message,
    /// Pauses the session ([`Chat::pause_desk`]).
    Pause,
    /// Resumes it.
    Resume,
    /// Opens [`DeskMenuItem::opens`].
    Open,
}

/// An item of a [`DeskMenu`]: its words, the line under them, and whether it can be pressed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct DeskMenuItem {
    /// What it does.
    pub action: DeskAction,
    /// Its words: `Message John`, `Pause`.
    pub title: String,
    /// Under them, muted: [`MESSAGE_UNDER`], [`PAUSE_UNDER`].
    pub under: String,
    /// Whether it can be pressed: Pause isn't while no watch is open.
    pub enabled: bool,
    /// The conversation Message and Open open: the coworker's own for Message, the
    /// session's for Open.
    pub opens: Option<String>,
}

/// The menu a working row's ⋯ opens ([`Chat::desk_menu`]), as a sheet: the row's face, name
/// and what it's doing, then its items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct DeskMenu {
    /// The row it's for: a coworker's nick, or a hot desk's conversation.
    pub key: String,
    /// Its face, as on its row.
    pub face: Face,
    /// Its name: `John`, `checkout-form`.
    pub name: String,
    /// Beside the name, muted: `Fixer · Henrik's Claude Code`, `Yours · Codex`.
    pub under: String,
    /// What it's doing, or `Paused by you`, in its desk's colour.
    pub doing: String,
    /// How its desk stands, which colours [`DeskMenu::doing`].
    pub desk: DeskState,
    /// Message, Pause or Resume (for its own person only), and Open, in that order.
    pub items: Vec<DeskMenuItem>,
}

/// The session a row stands for: its machine and id, and the coworker it sits as, if any.
struct Seated {
    machine: String,
    session: String,
    coworker: Option<String>,
    /// Its tool, as its desk's seat says it.
    tool: Option<String>,
}

impl Chat {
    /// The session the row `key` stands for: the one at coworker `key`'s desk, or the hot
    /// desk whose conversation is `key`. `None` for an empty desk or no such row.
    fn seated(&self, key: &str) -> Option<Seated> {
        if self.coworker_nicks().contains(&fold(key)) {
            let holder = self.role_seat(key)?.holder?;
            return Some(Seated {
                machine: holder.machine,
                tool: holder.session.tool,
                session: holder.session.session,
                coworker: Some(fold(key)),
            });
        }
        let (machine, session) = conversation::agent(key)?;
        Some(Seated {
            machine: machine.to_owned(),
            session: session.to_owned(),
            coworker: None,
            tool: None,
        })
    }

    /// Whether the agent session `session` on the machine `nick` is paused, as its machine
    /// says ([`zirc_proto::session::Live::paused`]).
    #[must_use]
    pub fn paused(&self, nick: &str, session: &str) -> bool {
        self.live_of(nick)
            .is_some_and(|live| live.paused.contains_key(session))
    }

    /// What a paused session's row says, in place of what it's doing: `Paused by you`, or
    /// `Paused by Henrik` on someone else's. `None` while it isn't paused.
    pub(super) fn paused_words(&self, nick: &str, session: &str) -> Option<String> {
        if !self.paused(nick, session) {
            return None;
        }
        let by = self
            .machine_owner(nick)
            .map_or_else(|| fold(nick), |owner| self.owner_name(owner));
        Some(zirc_proto::pause::paused_by(&by))
    }

    /// Whether the machine `nick` is ours, as the server says whose it is: only its owner
    /// pauses its sessions.
    fn ours_to_pause(&self, nick: &str) -> bool {
        self.machine_owner(nick)
            .is_some_and(|owner| owner.eq_ignore_ascii_case(self.me()))
    }

    /// The menu the working row `key` opens with its ⋯: a coworker at its desk, by nick, or
    /// a hot desk, by conversation. `None` for an empty desk or no such row.
    #[must_use]
    pub fn desk_menu(&self, key: &str) -> Option<DeskMenu> {
        let seated = self.seated(key)?;
        let Seated {
            machine,
            session,
            coworker,
            tool,
        } = &seated;
        let conversation = agent_channel(machine, session);
        let office = self.office(chrono::FixedOffset::east_opt(0)?);
        let (face, name, under, doing, desk) = if let Some(nick) = coworker {
            let card = office.coworkers.iter().find(|card| card.nick == *nick)?;
            let whose = self.whose_session(machine, session, tool.as_deref());
            let under = [card.job.clone(), Some(whose)]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
            (
                card.face.clone(),
                card.name.clone(),
                under,
                card.state.clone(),
                card.desk,
            )
        } else {
            let desk = office
                .hot_desks
                .iter()
                .chain(&office.hot_desks_folded)
                .find(|desk| desk.conversation == conversation)?;
            let paused = self.paused(machine, session);
            let state = match (paused, desk.online) {
                (true, _) => DeskState::Paused,
                (false, true) => DeskState::Working,
                (false, false) => DeskState::Idle,
            };
            (
                desk.face.clone(),
                desk.name.clone(),
                desk.whose.clone(),
                desk.doing.clone().unwrap_or_default(),
                state,
            )
        };
        let mut items = vec![DeskMenuItem {
            action: DeskAction::Message,
            title: format!("Message {name}"),
            under: MESSAGE_UNDER.to_owned(),
            enabled: true,
            opens: Some(coworker.clone().unwrap_or_else(|| conversation.clone())),
        }];
        if self.ours_to_pause(machine) {
            let watching = self.listening(machine, session) == Some(Listening::Online);
            items.push(if self.paused(machine, session) {
                DeskMenuItem {
                    action: DeskAction::Resume,
                    title: RESUME.to_owned(),
                    under: RESUME_UNDER.to_owned(),
                    enabled: true,
                    opens: None,
                }
            } else {
                DeskMenuItem {
                    action: DeskAction::Pause,
                    title: PAUSE.to_owned(),
                    under: if watching { PAUSE_UNDER } else { NOT_WATCHING }.to_owned(),
                    enabled: watching,
                    opens: None,
                }
            });
        }
        items.push(DeskMenuItem {
            action: DeskAction::Open,
            title: OPEN.to_owned(),
            under: OPEN_UNDER.to_owned(),
            enabled: true,
            opens: Some(conversation),
        });
        Some(DeskMenu {
            key: key.to_owned(),
            face,
            name,
            under,
            doing,
            desk,
            items,
        })
    }

    /// Whose session `session` on `machine` is and its tool, `seated` as its desk says, as
    /// a coworker's page says it: `Henrik's Claude Code`, `your Codex`.
    fn whose_session(&self, machine: &str, session: &str, seated: Option<&str>) -> String {
        let owner = self
            .machine_owner(machine)
            .map_or_else(|| fold(machine), |owner| self.owner_name(owner));
        let tool = self
            .session_tools
            .get(session)
            .map(String::as_str)
            .or(seated);
        super::speaker::whose_tool(&owner, tool)
    }

    /// The lines that pause the session the row `key` stands for, or resume it when `paused`
    /// is false: an `x-zirc/pause` to its machine's nick, which its zircd takes only from
    /// the machine's owner. None for a row that isn't ours to pause.
    #[must_use]
    pub fn pause_desk(&self, key: &str, paused: bool) -> Vec<Line> {
        let Some(Seated {
            machine, session, ..
        }) = self.seated(key)
        else {
            return Vec::new();
        };
        if !self.ours_to_pause(&machine) {
            return Vec::new();
        }
        // Debug, as its machine's live list is, so apps show no line for it: what it did
        // shows on the session's row.
        let message = Message {
            level: Some(Level::Debug),
            ..Message::new("", Pause { session, paused }.event())
        };
        let Ok(body) = zirc_proto::encode(&message) else {
            return Vec::new();
        };
        vec![Line::new("PRIVMSG", [machine, body])]
    }
}
