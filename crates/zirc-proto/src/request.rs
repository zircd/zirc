//! A question an agent session puts to its person, answered by a reaction: `x-zirc/request`,
//! a [`crate::Event::Custom`] in `#decisions` whose data is a [`Request`] and the
//! [`crate::card::Card`] apps draw for it. The person answers with 👍 (yes), 👎 (no) or ❓
//! (tell me more) on it, from any app. Only a reaction the server says their own account
//! gave, on the request's own message, counts: never a nick, and never a reaction from
//! anyone else, the requesting machine's sessions included. A later reaction replaces an
//! earlier one; taking it back leaves the request open again. The requesting machine posts
//! the request again with its [`State`], which apps draw in place of the first, so
//! `#decisions` reads as a queue of what's open.
//!
//! A yes to an [`Request::irreversible`] request is [`State::Held`] for [`HOLD_SECONDS`]
//! before it counts: a 👎, or taking the 👍 back, stops it meanwhile. A ❓ is answered by the
//! asking session in replies under the request (`+draft/reply` to its message): the person's
//! own replies there reach that session, and their reaction on any of them answers as one
//! on the request does.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::Event;
use crate::card::{Action, Card, Field, State as CardState};

/// The custom event kind a request is sent as.
pub const KIND: &str = "x-zirc/request";

/// The organisation's channel requests are asked in.
pub const CHANNEL: &str = "#decisions";

/// How long a yes to an irreversible request waits before it counts, in seconds.
pub const HOLD_SECONDS: u64 = 60;

/// What the person is asked, by whom, and what each answer does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(optional_fields, rename = "DecisionRequest")]
#[schemars(rename = "DecisionRequest")]
pub struct Request {
    /// Its id, unique to the requesting machine: `<machine>/<n>`.
    pub id: String,
    /// The question, one line.
    pub question: String,
    /// What a yes does.
    pub yes: String,
    /// What a no does.
    pub no: String,
    /// The answer the session recommends, shown first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommend: Option<Answer>,
    /// Whether acting on a yes can't be undone: then a yes waits a while, which a no stops.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub irreversible: bool,
    /// The session asking.
    pub session: String,
    /// Whose word it needs: the requesting machine's owner, by their handle, whose account
    /// alone can answer.
    pub by: String,
    /// Where it stands.
    #[serde(default)]
    pub state: State,
}

/// An answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[ts(rename = "DecisionAnswer")]
#[schemars(rename = "DecisionAnswer")]
pub enum Answer {
    /// 👍 ✅ 🙌
    Yes,
    /// 👎 ❌
    No,
    /// ❓ 🤔 👀: tell me more.
    More,
}

/// Where a request stands.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(tag = "state", rename_all = "lowercase")]
#[ts(rename = "DecisionRequestState")]
#[schemars(rename = "DecisionRequestState")]
pub enum State {
    /// Waiting for an answer.
    #[default]
    Open,
    /// A yes to an irreversible request, given `at` with `emoji`, which counts `until`
    /// (both RFC 3339) unless a 👎, or taking the 👍 back, stops it first.
    Held {
        /// The reaction that gave it.
        emoji: String,
        /// When, in RFC 3339.
        at: String,
        /// When it counts, in RFC 3339.
        until: String,
    },
    /// Answered, `at` (RFC 3339), with `emoji`.
    Answered {
        /// The answer.
        answer: Answer,
        /// The reaction that gave it: none when it was taken as recommended.
        emoji: String,
        /// When, in RFC 3339.
        at: String,
        /// Whether nobody gave it: the session's recommendation, taken at once as the
        /// person's machine is set to ([`Request::recommend`]). Their reaction on the
        /// request still says otherwise, whenever it comes.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        #[ts(as = "Option<bool>", optional)]
        recommended: bool,
    },
}

/// The answer an emoji gives, if it gives one: 👍 ✅ 🙌 yes, 👎 ❌ no, ❓ 🤔 👀 tell me more,
/// whatever skin tone or presentation it's sent with.
#[must_use]
pub fn answer_of(emoji: &str) -> Option<Answer> {
    // Skin tones (U+1F3FB to U+1F3FF) and the emoji presentation selector say how it looks,
    // not what it means.
    let bare: String = emoji
        .chars()
        .filter(|c| !matches!(*c as u32, 0x1F3FB..=0x1F3FF | 0xFE0F | 0xFE0E))
        .collect();
    match bare.as_str() {
        "👍" | "✅" | "🙌" => Some(Answer::Yes),
        "👎" | "❌" => Some(Answer::No),
        "❓" | "🤔" | "👀" => Some(Answer::More),
        _ => None,
    }
}

impl Answer {
    /// How it's said: `yes`, `no` or `tell me more`.
    #[must_use]
    pub fn said(self) -> &'static str {
        match self {
            Self::Yes => "yes",
            Self::No => "no",
            Self::More => "tell me more",
        }
    }
}

impl Request {
    /// The event that carries it, with the card apps draw for it. The review sandbox's seed
    /// (web/src/lib/sandbox-script.ts `ask`) copies this, [`Self::text`] and [`Self::card`]
    /// for an open request: change it there too.
    ///
    /// # Panics
    ///
    /// Never: a request and its card serialize to JSON.
    #[must_use]
    pub fn event(&self) -> Event {
        let data = serde_json::json!({ "request": self });
        Event::Custom {
            kind: KIND.to_owned(),
            data: self.card().into_data(data),
            text: self.text(),
        }
    }

    /// The request `event` carries, if it's one.
    #[must_use]
    pub fn of(event: &Event) -> Option<Self> {
        match event {
            Event::Custom { kind, data, .. } if kind == KIND => {
                serde_json::from_value(data.get("request")?.clone()).ok()
            }
            _ => None,
        }
    }

    /// What it says, for people and plain IRC clients.
    #[must_use]
    pub fn text(&self) -> String {
        match &self.state {
            State::Open => format!(
                "{}: {} 👍 yes: {} · 👎 no: {} · ❓ tell me more",
                self.by, self.question, self.yes, self.no
            ),
            State::Held { until, .. } => format!(
                "{} said yes: {} It's done at {until} unless they 👎 first",
                self.by, self.question
            ),
            State::Answered {
                answer,
                recommended: true,
                ..
            } => format!(
                "{}'s recommendation taken, {}: {} {} can still 👍 or 👎",
                self.session,
                answer.said(),
                self.question,
                self.by
            ),
            State::Answered { answer, .. } => {
                format!("{} answered {}: {}", self.by, answer.said(), self.question)
            }
        }
    }

    /// The card apps draw for it: the question, yes and no (the recommended one first), and
    /// how to answer, or the answer.
    #[must_use]
    pub fn card(&self) -> Card {
        let yes = Field {
            label: "👍 yes".into(),
            value: clip(&self.yes, 120),
        };
        let no = Field {
            label: "👎 no".into(),
            value: clip(&self.no, 120),
        };
        let fields = match self.recommend {
            Some(Answer::No) => vec![no, yes],
            _ => vec![yes, no],
        };
        let (state, note) = match &self.state {
            State::Open => (
                CardState::Pending,
                format!(
                    "{}: react 👍 yes · 👎 no · ❓ tell me more{}",
                    self.by,
                    if self.irreversible {
                        " (a yes can't be undone, so it waits a minute, which a 👎 stops)"
                    } else {
                        ""
                    }
                ),
            ),
            State::Held { until, .. } => (
                CardState::Running,
                format!(
                    "{} said yes; it's done at {} unless they 👎 or take the 👍 back",
                    self.by,
                    clock(until, true)
                ),
            ),
            State::Answered {
                answer: Answer::More,
                at,
                ..
            } => (
                CardState::Warning,
                format!(
                    "{} asked to know more at {}: {} answers beneath",
                    self.by,
                    clock(at, false),
                    self.session
                ),
            ),
            State::Answered {
                answer,
                at,
                recommended,
                ..
            } => (
                match answer {
                    Answer::Yes => CardState::Passed,
                    Answer::No => CardState::Failed,
                    Answer::More => CardState::Warning,
                },
                if *recommended {
                    format!(
                        "taken as recommended, {}, at {}; {} can still 👍 or 👎",
                        answer.said(),
                        clock(at, false),
                        self.by
                    )
                } else {
                    format!(
                        "{} answered {} at {}",
                        self.by,
                        answer.said(),
                        clock(at, false)
                    )
                },
            ),
        };
        Card {
            id: self.id.clone(),
            title: clip(&self.question, 80),
            state,
            about: Some(clip(&self.session, 64)),
            fields,
            links: Vec::new(),
            actions: Vec::<Action>::new(),
            note: Some(clip(&note, 120)),
        }
    }
}

/// `text` on one line, within `max` characters, cut with `…`.
fn clip(text: &str, max: usize) -> String {
    crate::clip(&text.replace(['\n', '\r'], " "), max)
}

/// An RFC 3339 time in UTC as people read it on a card, `14:32 UTC` (with seconds,
/// `14:32:04 UTC`), for a card's note is the same words for everyone; anything else as it is.
fn clock(at: &str, seconds: bool) -> String {
    let end = if seconds { 19 } else { 16 };
    match (at.get(10..11), at.get(11..end)) {
        (Some("T"), Some(time)) if at.ends_with('Z') => format!("{time} UTC"),
        _ => at.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Request {
        Request {
            id: "mattias-mbp/1".into(),
            question: "Add a second CI node?".into(),
            yes: "Builds run in half the time; £40 a month more".into(),
            no: "Builds stay as they are".into(),
            recommend: Some(Answer::Yes),
            irreversible: false,
            session: "cc-1".into(),
            by: "mattias".into(),
            state: State::Open,
        }
    }

    #[test]
    fn an_emoji_answers_whatever_its_skin_tone() {
        for (emoji, answer) in [
            ("👍", Answer::Yes),
            ("👍🏽", Answer::Yes),
            ("✅", Answer::Yes),
            ("🙌", Answer::Yes),
            ("👎", Answer::No),
            ("❌", Answer::No),
            ("❓", Answer::More),
            ("🤔", Answer::More),
            ("👀", Answer::More),
        ] {
            assert_eq!(answer_of(emoji), Some(answer), "{emoji}");
        }
        assert_eq!(answer_of("🎉"), None);
    }

    #[test]
    fn travels_as_a_custom_event_with_a_card_apps_can_draw() {
        let request = request();
        let event = request.event();
        assert_eq!(Request::of(&event), Some(request.clone()));
        let card = Card::in_event(&event).expect("a card apps can draw");
        assert_eq!(card.id, "mattias-mbp/1");
        assert_eq!(card.state, CardState::Pending);
        assert_eq!(card.fields[0].label, "👍 yes");
        assert!(event.human_text().contains("👍 yes: Builds run"));
        let answered = Request {
            state: State::Answered {
                answer: Answer::No,
                emoji: "👎".into(),
                at: "2026-11-05T10:00:00Z".into(),
                recommended: false,
            },
            ..request
        };
        assert_eq!(answered.card().state, CardState::Failed);
        assert_eq!(
            answered.card().note.as_deref(),
            Some("mattias answered no at 10:00 UTC")
        );
        assert_eq!(
            answered.text(),
            "mattias answered no: Add a second CI node?"
        );
    }

    #[test]
    fn a_held_yes_says_when_it_counts_and_how_to_stop_it() {
        let held = Request {
            irreversible: true,
            state: State::Held {
                emoji: "👍".into(),
                at: "2026-11-05T10:00:00Z".into(),
                until: "2026-11-05T10:01:00Z".into(),
            },
            ..request()
        };
        let card = held.card();
        assert_eq!(card.state, CardState::Running);
        assert!(
            card.note
                .as_deref()
                .unwrap()
                .contains("10:01:00 UTC unless they 👎"),
            "{card:?}"
        );
        assert!(held.text().contains("unless they 👎 first"));
        let open = Request {
            irreversible: true,
            ..request()
        };
        assert!(open.card().note.unwrap().contains("which a 👎 stops"));
    }

    #[test]
    fn one_taken_as_recommended_says_so_and_that_its_person_can_still_answer() {
        let taken = Request {
            recommend: Some(Answer::Yes),
            state: State::Answered {
                answer: Answer::Yes,
                emoji: String::new(),
                at: "2026-11-12T14:32:04Z".into(),
                recommended: true,
            },
            ..request()
        };
        assert_eq!(
            taken.card().note.as_deref(),
            Some("taken as recommended, yes, at 14:32 UTC; mattias can still 👍 or 👎")
        );
        assert_eq!(taken.card().state, CardState::Passed);
        assert!(
            taken.text().contains("recommendation taken, yes"),
            "{}",
            taken.text()
        );
        // One from before it could be says nothing of it, and reads as their own answer.
        let old = r#"{"state":"answered","answer":"no","emoji":"👎","at":"2026-11-12T14:32:04Z"}"#;
        let old: State = serde_json::from_str(old).unwrap();
        assert!(matches!(
            old,
            State::Answered {
                recommended: false,
                ..
            }
        ));
        assert!(!serde_json::to_string(&old).unwrap().contains("recommended"));
    }
}
