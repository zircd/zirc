//! Getting started's prompt in an app: which one shows now, where, and its words, decided
//! here for every app from the person's steps as zirc.dev says they stand
//! ([`zirc_proto::onboarding::Progress`]) and from what the chat knows: which agent of
//! theirs listens, and which has answered them.
//!
//! One prompt at a time, for the next step alone, where the person is; gone once its step is
//! done, as the next [`Progress`] says. Which step is next is zirc-proto's to say
//! ([`zirc_proto::onboarding::progress`]): nothing here decides it again.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use zirc_proto::ci;
use zirc_proto::onboarding::{INVITE_PATH, Progress, State, Today, id};

use super::flat::RichSpan;
use super::lines::Keys;
use super::rich::Span;
use super::words::{DISMISS, INVITE, NOT_NOW};
use super::{Callout, Chat, EntryKind, agent};

/// What the card over an agent's conversation is headed, in every app.
pub const NEXT_STEP_LABEL: &str = "Zirc · next step";

/// What starts a next-step card's button's action, as an app hands it back
/// ([`super::Callout::action`]): `prompt:ci`, the step's id after it.
pub const PROMPT_ACTION: &str = "prompt:";

/// The theme role of a prompt atop Messages, as the mocks tint it.
const MESSAGES_TONE: &str = "signal";

/// The theme role of a prompt in an agent's conversation: the step's next, green.
const NEXT_STEP_TONE: &str = "ok";

/// Where a prompt shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase", tag = "at")]
#[ts(rename_all = "camelCase")]
pub enum PromptPlace {
    /// At the top of Messages, under anything waiting for the person.
    Messages,
    /// In the conversation called `name`, an agent's, under the message keyed `after`.
    Conversation {
        /// The conversation.
        name: String,
        /// The key of the message it goes under: the agent's first answer.
        after: String,
    },
}

/// What a prompt's button does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase", tag = "does")]
#[ts(rename_all = "camelCase")]
pub enum PromptAction {
    /// Opens the conversation called `name`, to write in.
    Open {
        /// The conversation.
        name: String,
    },
    /// Says `text` in the conversation called `name`, for the person.
    Say {
        /// The conversation.
        name: String,
        /// What's said.
        text: String,
    },
    /// Opens zirc.dev's page at `path`, for the organisation the app is signed in to
    /// ([`INVITE_PATH`]).
    Page {
        /// The page's path on zirc.dev.
        path: String,
    },
}

/// A prompt's second button, that puts it away: logged as dismissed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct PromptLater {
    /// What it says: `Not now`.
    pub label: String,
    /// The days after which it's back, the first time it's put away; the second time, and
    /// with none, it's gone for good.
    pub back_after_days: Option<u32>,
}

/// How long a prompt the person put away stays away, as an app keeps it for its step.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutAway {
    /// How many times it was put away.
    pub times: u32,
    /// Until when (unix seconds) it stays away; none, for good.
    pub until: Option<i64>,
}

impl PutAway {
    /// Whether it stays away at `now` (unix seconds).
    #[must_use]
    pub fn hides(&self, now: i64) -> bool {
        self.times > 0 && self.until.is_none_or(|until| now < until)
    }

    /// Put away once more at `now` with `later`: back after its days the first time, then
    /// gone for good.
    #[must_use]
    pub fn again(self, later: &PromptLater, now: i64) -> Self {
        let back = later
            .back_after_days
            .filter(|_| self.times == 0)
            .map(|days| now + i64::from(days) * 24 * 60 * 60);
        Self {
            times: self.times + 1,
            until: back,
        }
    }
}

/// A prompt's button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct PromptButton {
    /// What it says: `Message tls-rotation`.
    pub label: String,
    /// What it does.
    pub action: PromptAction,
}

/// Getting started's prompt as an app shows it now: every app draws this, deciding nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct Prompt {
    /// The step it's for, by its id ([`zirc_proto::onboarding::Step::id`]): what's logged
    /// when it's shown, acted on or put away.
    pub step: String,
    /// Where it shows.
    pub place: PromptPlace,
    /// Over an agent's conversation, what it's headed ([`NEXT_STEP_LABEL`]).
    pub label: Option<String>,
    /// The theme role it's tinted with: `signal` atop Messages, `ok` in a conversation, as
    /// the mocks have them.
    pub tone: String,
    /// Its first words, in bold: `tls-rotation is listening.`
    pub title: Option<String>,
    /// What it says.
    pub body: String,
    /// Its one action, until the person has taken it.
    pub button: Option<PromptButton>,
    /// Atop Messages, the button that puts it away.
    pub later: Option<PromptLater>,
    /// Once they have, what it says in the button's place: `✓ You asked tls-rotation to do
    /// it · 09:10`.
    pub done: Option<String>,
}

/// What an app knows that the chat doesn't.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PromptContext {
    /// Whether the app is a phone's: the phone's step is done from one alone.
    pub phone: bool,
    /// When the person took the next step's prompt's action, on the app's clock, if they
    /// have.
    pub acted: Option<NaiveDateTime>,
    /// What their agents did today, as zirc.dev counts it, once asked ([`wants_today`]).
    pub today: Option<Today>,
}

/// The days the invite card stays away the first time it's put away.
const INVITE_BACK_AFTER_DAYS: u32 = 7;

/// What a teammate's step is for, as zirc-proto words it for every surface
/// ([`zirc_proto::onboarding::Step::about`]).
fn teammate_about() -> String {
    zirc_proto::onboarding::STEPS
        .iter()
        .find(|step| step.id == id::TEAMMATE)
        .map_or_else(String::new, |step| step.about.to_owned())
}

/// Whether an app should ask zirc.dev what the person's agents did today
/// ([`zirc_proto::onboarding::TODAY_PATH`]): only once the invite card may show.
#[must_use]
pub fn wants_today(progress: &Progress) -> bool {
    progress
        .steps
        .iter()
        .find(|step| step.next)
        .is_some_and(|next| matches!(next.id.as_str(), id::FIRST_OVERLAP | id::TEAMMATE))
}

impl Chat {
    /// Keeps the person's steps of getting started as zirc.dev says they stand, and what the
    /// app knows of them, for the next step's card in a conversation ([`Chat::lines`]).
    pub fn set_prompting(&mut self, progress: Option<Progress>, context: PromptContext) {
        self.prompting = progress.map(|progress| (progress, context));
    }

    /// The next step's card for the conversation called `name`, when its prompt is in it: the
    /// key of the message it goes under, and what it says, as a callout every app draws.
    pub(super) fn next_step_card(&self, name: &str) -> Option<(String, Callout)> {
        let (progress, context) = self.prompting.as_ref()?;
        let prompt = self.prompt(progress, *context)?;
        let PromptPlace::Conversation { name: there, after } = &prompt.place else {
            return None;
        };
        if !there.eq_ignore_ascii_case(name) {
            return None;
        }
        let span = |text: String, bold: bool| -> RichSpan {
            Span {
                text,
                bold,
                ..Span::default()
            }
            .into()
        };
        let mut spans = Vec::new();
        if let Some(title) = prompt.title {
            spans.push(span(format!("{title} "), true));
        }
        spans.push(span(prompt.body, false));
        if let Some(done) = prompt.done {
            spans.push(span(format!(" {done}"), true));
        }
        let callout = Callout {
            label: prompt.label.unwrap_or_else(|| NEXT_STEP_LABEL.to_owned()),
            spans,
            tone: prompt.tone,
            button: prompt.button.as_ref().map(|button| button.label.clone()),
            action: prompt
                .button
                .map(|_| format!("{PROMPT_ACTION}{}", prompt.step)),
            command: None,
            footer: None,
            heads: true,
        };
        Some((after.clone(), callout))
    }

    /// Getting started's prompt to show now, given the person's steps as `progress` says
    /// they stand: the next step's, where this app can prompt for it, or none.
    #[must_use]
    pub fn prompt(&self, progress: &Progress, context: PromptContext) -> Option<Prompt> {
        let next = progress.steps.iter().find(|step| step.next)?;
        if next.state != State::Available {
            return None;
        }
        match next.id.as_str() {
            id::PHONE => self.say_hi(context),
            id::CI => self.connect_ci(context),
            id::FIRST_WAKE => self.see_a_wake(context),
            // The first overlap asks nothing of the person, so while it waits, a teammate's
            // step is prompted, as is the teammate's own.
            id::FIRST_OVERLAP | id::TEAMMATE => Self::invite(progress, context),
            // The rest are prompted elsewhere: the dashboard, the terminal, #agents.
            _ => None,
        }
    }

    /// The phone's step: an agent of theirs listens, so say hi to it. On a phone only, where
    /// a message proves the step.
    fn say_hi(&self, context: PromptContext) -> Option<Prompt> {
        if !context.phone {
            return None;
        }
        let name = self.listening_agent()?;
        let agent = self.agent_name(&name);
        Some(Prompt {
            step: id::PHONE.to_owned(),
            place: PromptPlace::Messages,
            label: None,
            tone: MESSAGES_TONE.to_owned(),
            title: Some(format!("{agent} is listening.")),
            body: "Say hi: a message reaches it in seconds, wherever you are.".to_owned(),
            button: Some(PromptButton {
                label: format!("Message {agent}"),
                action: PromptAction::Open { name },
            }),
            later: Some(PromptLater {
                label: DISMISS.to_owned(),
                back_after_days: None,
            }),
            done: None,
        })
    }

    /// A teammate's step, atop Messages once the person's agents woke today: how many
    /// times, and the builds they fixed, as zirc.dev counts them, and Invite.
    fn invite(progress: &Progress, context: PromptContext) -> Option<Prompt> {
        let teammate = progress.steps.iter().find(|step| step.id == id::TEAMMATE)?;
        let today = context.today?;
        if teammate.state != State::Available || today.wakes == 0 {
            return None;
        }
        let woke = zirc_proto::times(today.wakes);
        let fixed = match today.fixed {
            0 => String::new(),
            1 => " and fixed a build".to_owned(),
            n => format!(" and fixed {n} builds"),
        };
        Some(Prompt {
            step: id::TEAMMATE.to_owned(),
            place: PromptPlace::Messages,
            label: None,
            tone: MESSAGES_TONE.to_owned(),
            title: Some(format!("Your agents woke {woke} today{fixed}.")),
            // The step's own words, as Get started and the terminal say it.
            body: teammate_about(),
            button: Some(PromptButton {
                label: INVITE.to_owned(),
                action: PromptAction::Page {
                    path: INVITE_PATH.to_owned(),
                },
            }),
            later: Some(PromptLater {
                label: NOT_NOW.to_owned(),
                back_after_days: Some(INVITE_BACK_AFTER_DAYS),
            }),
            done: None,
        })
    }

    /// The CI step, in the conversation of the agent that answered them: ask it to connect
    /// CI.
    fn connect_ci(&self, context: PromptContext) -> Option<Prompt> {
        let (name, after) = self.answering_agent()?;
        let agent = self.agent_name(&name);
        let done = context
            .acted
            .map(|at| format!("✓ You asked {agent} to do it · {}", super::clock(at)));
        Some(Prompt {
            step: id::CI.to_owned(),
            place: PromptPlace::Conversation {
                name: name.clone(),
                after,
            },
            label: Some(NEXT_STEP_LABEL.to_owned()),
            tone: NEXT_STEP_TONE.to_owned(),
            title: context
                .phone
                .then(|| format!("Your phone reaches {agent}.")),
            body: "Next, connect CI: then it wakes when a build of its fails, even with you \
                   away."
                .to_owned(),
            button: done.is_none().then(|| PromptButton {
                label: format!("Ask {agent} to do it"),
                action: PromptAction::Say {
                    name,
                    text: ci::CONNECT_ASK.to_owned(),
                },
            }),
            later: None,
            done,
        })
    }

    /// The first wake's step, in the same conversation: send a test failure.
    fn see_a_wake(&self, context: PromptContext) -> Option<Prompt> {
        let (name, after) = self.answering_agent()?;
        let done = context
            .acted
            .map(|at| format!("✓ Sent · {}. CI is running it.", super::clock(at)));
        Some(Prompt {
            step: id::FIRST_WAKE.to_owned(),
            place: PromptPlace::Conversation {
                name: name.clone(),
                after,
            },
            label: Some(NEXT_STEP_LABEL.to_owned()),
            tone: NEXT_STEP_TONE.to_owned(),
            title: None,
            body: "See a wake now, without waiting for a real failure.".to_owned(),
            // The agent runs it, where its CI's tools are ([`ci::test_failure_ask`]).
            button: done.is_none().then(|| PromptButton {
                label: "Send a test failure".to_owned(),
                action: PromptAction::Say {
                    name,
                    text: ci::test_failure_ask(),
                },
            }),
            later: None,
            done,
        })
    }

    /// What an agent session's conversation called `name` goes by.
    fn agent_name(&self, name: &str) -> String {
        agent(name).map_or_else(
            || name.to_owned(),
            |(_, session)| self.session_label(session),
        )
    }

    /// The conversation of an agent of ours that's listening, a watch of its open: the one
    /// said in last, else the first by name.
    fn listening_agent(&self) -> Option<String> {
        let mut listening: Vec<String> = self
            .live_sessions(|nick| self.ours(nick))
            .into_iter()
            .filter(|name| {
                agent(name).is_some_and(|(nick, session)| {
                    self.listening(nick, session) == Some(super::Listening::Online)
                })
            })
            .collect();
        listening.sort();
        listening
            .into_iter()
            .max_by_key(|name| self.last_said(name))
    }

    /// The conversation of an agent of ours that has answered us, and the key of its first
    /// answer: the conversation answered in last.
    fn answering_agent(&self) -> Option<(String, String)> {
        self.conversations
            .iter()
            .filter(|conversation| {
                agent(&conversation.name).is_some_and(|(nick, _)| self.ours(nick))
            })
            .filter_map(|conversation| {
                // Keyed as its line is ([`Keys`]), so an app finds the message it goes under.
                let mut keys = Keys::default();
                let mut asked = false;
                let answer = conversation.entries.iter().find_map(|entry| {
                    let ours = entry.from.eq_ignore_ascii_case(&self.me);
                    if asked && !ours && matches!(entry.kind, EntryKind::Zirc(_)) {
                        return Some(keys.of(entry));
                    }
                    keys.skip(entry);
                    asked |= ours;
                    None
                })?;
                Some((conversation.last_said(), conversation.name.clone(), answer))
            })
            .max_by_key(|(last, _, _)| *last)
            .map(|(_, name, key)| (name, key))
    }
}
