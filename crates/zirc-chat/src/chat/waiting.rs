//! What's waiting for the person's answer, in one place: the card every app pins at the top
//! of Messages, `WAITING FOR YOU · 2` ([`Chat::waiting_card`]). An outside reviewer running 14
//! agents found approvals scattered across conversations and the same thing asked twice, and
//! built a tracker of his own (22 Nov 2026).
//!
//! Two things wait (the user's choices, 24 Nov 2026):
//!
//! - an open request ([`zirc_proto::request`]) one of the person's own machines asked them in
//!   `#decisions`, as [`zirc_proto::request::waiting`] says: requests asking the same thing
//!   show once, and the answer goes to each;
//! - a question one of their own agents put to them directly (`zirc dm`): a conversation
//!   with an agent session whose newest line is the agent's and ends in a question mark. It
//!   leaves when they answer there, or the agent says something else.
//!
//! An item's buttons answer in place, as a reaction in `#decisions` does
//! ([`Chat::answer_waiting`]). An answer given isn't shown as taken: the item stays, saying
//! the answer was sent, until the requesting machine posts the request's card again as
//! answered. So nothing here reads as decided that a machine never heard.

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use zirc_proto::request::{self, Answer, Posted, Request, State};

use super::conversation::{Entry, EntryKind, agent};
use super::face::Face;
use super::list::SidebarSection;
use super::{Chat, Line};

/// What the card is called, said once for every app: its heading in capitals with the count
/// ([`WaitingCard::title`]), and the dashboard's panel as it is.
pub const WAITING_FOR_YOU: &str = "Waiting for you";

/// What an item's ⋯ button is called, over its menu ([`WaitingItem::menu`]).
pub const MORE_ANSWERS: &str = "More answers";

/// The card pinned at the top of Messages while something waits ([`Chat::waiting_card`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct WaitingCard {
    /// Its heading: `WAITING FOR YOU · 2`.
    pub title: String,
    /// What a screen reader calls it: `Waiting for you, 2`.
    pub label: String,
    /// What an item's ⋯ button is called, for a screen reader and its tooltip:
    /// [`MORE_ANSWERS`].
    pub more: String,
    /// What waits, the longest waiting first: never empty.
    pub items: Vec<WaitingItem>,
}

/// What an item of the card is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum WaitingKind {
    /// A yes or no an agent asked for (`zirc request`).
    Request,
    /// A question an agent put to the person directly (`zirc dm`).
    Question,
}

/// One thing waiting, as every app draws it: the agent's face and name, the question in
/// bold, what each answer does, a note, then its buttons in a row and a ⋯ menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct WaitingItem {
    /// What [`Chat::answer_waiting`] takes: a request's id, or the conversation of a question.
    pub key: String,
    /// What it is.
    pub kind: WaitingKind,
    /// The agent asking.
    pub face: Face,
    /// Who asks, small over the question: `tls-rotation`, or with the others asking the
    /// same, `tls-rotation · also asked by dispatcher`.
    pub agent: String,
    /// The question, in bold.
    pub question: String,
    /// What each answer does, the recommended first: `Yes` it's revoked, `No` it stays.
    /// Empty for a question.
    pub does: Vec<WaitingDoes>,
    /// A line under those, muted: that a yes can't be undone, when a held yes counts, that
    /// an answer was sent and isn't taken yet.
    pub note: Option<String>,
    /// Its buttons, in a row: Yes and No, Stop for a held yes, Reply for a question.
    pub buttons: Vec<WaitingButton>,
    /// What its ⋯ menu offers besides: Tell me more, Your call. Empty for a question.
    pub menu: Vec<WaitingButton>,
    /// The conversation pressing the item opens: `#decisions`, or the agent's.
    pub opens: String,
    /// The message there to open at ([`Chat::open_at`]), by the server's id.
    pub at: Option<String>,
}

/// What an answer does, as an item says under its question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct WaitingDoes {
    /// The answer, as its button says it: `Yes`.
    pub label: String,
    /// What it does.
    pub text: String,
}

/// What a button of an item does ([`Chat::answer_waiting`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum WaitingAction {
    /// Answers yes.
    Yes,
    /// Answers no.
    No,
    /// Asks the agent to say more, which it does under the request in `#decisions`.
    More,
    /// Leaves it to the agent.
    Yours,
    /// Stops a held yes before it counts: a no.
    Stop,
    /// Opens the agent's conversation to answer in: the app opens [`WaitingItem::opens`],
    /// and nothing is sent.
    Reply,
}

impl WaitingAction {
    /// What its button says.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Yes => "Yes",
            Self::No => "No",
            Self::More => "Tell me more",
            Self::Yours => "Your call",
            Self::Stop => "Stop",
            Self::Reply => "Reply",
        }
    }

    /// The answer it gives a request; none for [`Self::Reply`].
    fn answer(self) -> Option<Answer> {
        match self {
            Self::Yes => Some(Answer::Yes),
            Self::No | Self::Stop => Some(Answer::No),
            Self::More => Some(Answer::More),
            Self::Yours => Some(Answer::Yours),
            Self::Reply => None,
        }
    }
}

/// A button of an item, or an entry of its menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct WaitingButton {
    /// What it says: `Yes`.
    pub label: String,
    /// What pressing it does.
    pub action: WaitingAction,
    /// Whether it's the filled one: the answer the agent recommends, else yes.
    pub primary: bool,
    /// Whether the person gave this answer already and its machine hasn't taken it yet:
    /// drawn pressed.
    pub sent: bool,
}

impl WaitingButton {
    fn of(action: WaitingAction, primary: bool, sent: Option<Answer>) -> Self {
        Self {
            label: action.label().to_owned(),
            action,
            primary,
            sent: sent.is_some() && sent == action.answer(),
        }
    }
}

impl Chat {
    /// The card over the list while `tab` shows Messages, or both halves, its times on a
    /// clock `offset` from UTC, as [`Chat::lines`] has them: `None` while nothing waits, and
    /// over Channels, where no app draws it.
    #[must_use]
    pub fn waiting_card(
        &self,
        tab: Option<SidebarSection>,
        offset: FixedOffset,
    ) -> Option<WaitingCard> {
        if tab == Some(SidebarSection::Channels) {
            return None;
        }
        let items = self.waiting(offset);
        if items.is_empty() {
            return None;
        }
        Some(WaitingCard {
            title: format!("{} · {}", WAITING_FOR_YOU.to_uppercase(), items.len()),
            label: format!("{WAITING_FOR_YOU}, {}", items.len()),
            more: MORE_ANSWERS.to_owned(),
            items,
        })
    }

    /// What waits for the person's answer: their agents' open requests, then the questions
    /// their agents put to them directly, each the longest waiting first, times on a clock
    /// `offset` from UTC.
    #[must_use]
    pub fn waiting(&self, offset: FixedOffset) -> Vec<WaitingItem> {
        let mut items: Vec<WaitingItem> = request::waiting(self.requests_heard(), &self.me)
            .iter()
            .map(|waiting| self.request_item(waiting, offset))
            .collect();
        items.extend(self.questions());
        items
    }

    /// The requests heard in `#decisions` from the person's own machines, oldest first: only
    /// what the server says one of their machines sent ([`Chat::ours`]), never a card
    /// anyone else posted in its likeness.
    fn requests_heard(&self) -> Vec<Posted> {
        let Some(decisions) = self.conversation(request::CHANNEL) else {
            return Vec::new();
        };
        decisions
            .entries
            .iter()
            .filter(|entry| self.ours(&entry.from))
            .filter_map(|entry| {
                let EntryKind::Zirc(message) = &entry.kind else {
                    return None;
                };
                Some(Posted {
                    msgid: entry.id.clone()?,
                    request: Request::of(&message.event)?,
                })
            })
            .collect()
    }

    /// The machine that asked the request whose first message is `msgid`.
    fn asker(&self, msgid: &str) -> Option<&str> {
        self.conversation(request::CHANNEL)?
            .entry(msgid)
            .map(|entry| entry.from.as_str())
    }

    /// The answer the person gave the request at `msgid` that its machine hasn't taken: their
    /// own reaction on it, while its card still says it's open.
    fn answer_sent(&self, msgid: &str) -> Option<Answer> {
        self.conversation(request::CHANNEL)?
            .reactions(msgid)
            .iter()
            .filter(|reaction| reaction.by(&self.me))
            .find_map(|reaction| request::answer_of(&reaction.emoji))
    }

    fn request_item(&self, waiting: &request::Waiting, offset: FixedOffset) -> WaitingItem {
        let asked = &waiting.request;
        let machine = self.asker(&waiting.msgid).unwrap_or_default().to_owned();
        let row = self.session_row(&machine, &asked.session, machine.clone());
        let mut agent = row.label;
        if !waiting.also.is_empty() {
            let others: Vec<String> = waiting
                .also
                .iter()
                .map(|(other, _)| self.session_label(&other.session))
                .collect();
            agent = format!("{agent} · also asked by {}", others.join(", "));
        }
        let yes = WaitingDoes {
            label: WaitingAction::Yes.label().to_owned(),
            text: asked.yes.clone(),
        };
        let no = WaitingDoes {
            label: WaitingAction::No.label().to_owned(),
            text: asked.no.clone(),
        };
        let no_first = asked.recommend == Some(Answer::No);
        // An answer sent counts as such only while the card says it's open: a held yes and a
        // "tell me more" are taken, and say so themselves.
        let sent = (asked.state == State::Open)
            .then(|| self.answer_sent(&waiting.msgid))
            .flatten();
        let irreversible =
            asked.irreversible || waiting.also.iter().any(|(other, _)| other.irreversible);
        let (note, buttons) = match &asked.state {
            State::Held { until, .. } => (
                Some(format!(
                    "Your yes counts at {} unless you stop it",
                    clock(until, offset)
                )),
                vec![WaitingButton::of(WaitingAction::Stop, false, None)],
            ),
            state => {
                let note = match (state, sent) {
                    (State::Answered { .. }, _) => Some(format!(
                        "You asked for more: {} answers in {}",
                        self.session_label(&asked.session),
                        request::CHANNEL
                    )),
                    (_, Some(answer)) => Some(format!(
                        "Your {} is sent: waiting for {machine} to take it",
                        answer.said()
                    )),
                    (_, None) if irreversible => {
                        Some("A yes can't be undone: it waits a minute, which Stop ends".to_owned())
                    }
                    _ => None,
                };
                let yes = WaitingButton::of(WaitingAction::Yes, !no_first, sent);
                let no = WaitingButton::of(WaitingAction::No, no_first, sent);
                (
                    note,
                    if no_first {
                        vec![no, yes]
                    } else {
                        vec![yes, no]
                    },
                )
            }
        };
        let mut menu = Vec::new();
        if matches!(asked.state, State::Open) {
            menu.push(WaitingButton::of(WaitingAction::More, false, sent));
        }
        if waiting.offers_yours() && !matches!(asked.state, State::Held { .. }) {
            menu.push(WaitingButton::of(WaitingAction::Yours, false, sent));
        }
        WaitingItem {
            key: asked.id.clone(),
            kind: WaitingKind::Request,
            face: self.face(&row.avatar),
            agent,
            question: asked.question.clone(),
            does: if no_first {
                vec![no, yes]
            } else {
                vec![yes, no]
            },
            note,
            buttons,
            menu,
            opens: request::CHANNEL.to_owned(),
            at: Some(waiting.msgid.clone()),
        }
    }

    /// The questions the person's own agents put to them directly and they haven't answered:
    /// each conversation with one of their agent sessions, still known to its machine, whose
    /// newest line is the agent's own words ending in a question mark. The longest waiting
    /// first.
    fn questions(&self) -> Vec<WaitingItem> {
        let mut asked: Vec<(&Entry, WaitingItem)> = self
            .conversations()
            .iter()
            .filter_map(|conversation| {
                let (nick, session) = agent(&conversation.name)?;
                if !self.ours(nick) || self.session_live(nick, session) == Some(false) {
                    return None;
                }
                let entry = conversation
                    .entries
                    .iter()
                    .rev()
                    .find(|entry| entry.is_shown_in_list())?;
                if entry.from.eq_ignore_ascii_case(&self.me) {
                    return None;
                }
                let question = asks(entry)?;
                let row = self.session_row(nick, session, nick.to_owned());
                let item = WaitingItem {
                    key: conversation.name.clone(),
                    kind: WaitingKind::Question,
                    face: self.face(&row.avatar),
                    agent: row.label,
                    question,
                    does: Vec::new(),
                    note: None,
                    buttons: vec![WaitingButton::of(WaitingAction::Reply, true, None)],
                    menu: Vec::new(),
                    opens: conversation.name.clone(),
                    at: entry.id.clone(),
                };
                Some((entry, item))
            })
            .collect();
        asked.sort_by_key(|(entry, _)| entry.time);
        asked.into_iter().map(|(_, item)| item).collect()
    }

    /// The lines that give the waiting item `key` the answer `action`: the answer's reaction
    /// on the request's message in `#decisions`, and on each other request asking the same,
    /// as reacting there by hand does. Its machine takes it from there, and the item leaves
    /// once the machine's card says so. Nothing for [`WaitingAction::Reply`], for an answer
    /// the item doesn't offer, one given already, or an item that no longer waits.
    #[must_use]
    pub fn answer_waiting(&self, key: &str, action: WaitingAction) -> Vec<Line> {
        let Some(answer) = action.answer() else {
            return Vec::new();
        };
        let waiting = request::waiting(self.requests_heard(), &self.me);
        let Some(waiting) = waiting.iter().find(|waiting| waiting.request.id == key) else {
            return Vec::new();
        };
        if answer == Answer::Yours && !waiting.offers_yours() {
            return Vec::new();
        }
        let emoji = answer.reaction();
        let given = |msgid: &str| {
            self.conversation(request::CHANNEL).is_some_and(|c| {
                c.reactions(msgid)
                    .iter()
                    .any(|r| r.by(&self.me) && request::answer_of(&r.emoji) == Some(answer))
            })
        };
        waiting
            .msgids()
            .filter(|msgid| !given(msgid))
            .flat_map(|msgid| self.react(request::CHANNEL, msgid, emoji))
            .collect()
    }
}

/// The question `entry` asks, when it's an agent's own words ending in a question mark: its
/// last paragraph that does, on one line, as the card shows it.
fn asks(entry: &Entry) -> Option<String> {
    let text = match &entry.kind {
        EntryKind::Message(text) | EntryKind::Notice(text) => text.as_str(),
        EntryKind::Zirc(message) => match &message.event {
            zirc_proto::Event::Text { body } => body.as_str(),
            _ => return None,
        },
        _ => return None,
    };
    let text = super::strip_formatting(text);
    let text = text.trim_end();
    if !text.ends_with(['?', '？']) {
        return None;
    }
    let last = text.rsplit("\n\n").next().unwrap_or(text);
    Some(last.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// An RFC 3339 time on the app's clock, `offset` from UTC, as every app writes a time
/// ([`super::clock`]): `14:32`. Anything else as it is.
fn clock(at: &str, offset: FixedOffset) -> String {
    DateTime::parse_from_rfc3339(at).map_or_else(
        |_| at.to_owned(),
        |at| super::clock(at.with_timezone(&offset).naive_local()),
    )
}
