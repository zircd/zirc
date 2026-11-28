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
//!
//! A request its session no longer needs an answer to is [`State::Withdrawn`]: the session
//! took it back, ended, or finished the task it asked for.
//!
//! What waits for a person's answer is decided once, here ([`waiting`]): every app's
//! "Waiting for you" and the dashboard's read it, and none works it out again.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::Event;
use crate::card::{Card, Field, State as CardState};

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
    /// 🤷: the session's call, to go ahead on its own judgement. Never an answer to a request
    /// that can't be undone, which a person decides.
    Yours,
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
    /// Taken back unanswered, `at` (RFC 3339): no answer is needed any more, and none counts.
    Withdrawn {
        /// When, in RFC 3339.
        at: String,
        /// Why, in a few words, as the card says: [`Withdrawal::said`].
        why: Withdrawal,
    },
}

/// Why a request was withdrawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[ts(rename = "DecisionWithdrawal")]
#[schemars(rename = "DecisionWithdrawal")]
pub enum Withdrawal {
    /// Its session took it back (`zirc request --withdraw`).
    Asked,
    /// Its session finished, or dropped, the task it asked for.
    Done,
    /// Its session ended, or has been away as long as its row stays listed
    /// ([`crate::session::AWAY_FOLD_SECS`]).
    Gone,
}

impl Withdrawal {
    /// How a card says it, after the session's name: `withdrew it`, `finished its task`,
    /// `is gone`.
    #[must_use]
    pub fn said(self) -> &'static str {
        match self {
            Self::Asked => "withdrew it",
            Self::Done => "finished its task",
            Self::Gone => "is gone",
        }
    }
}

/// The answer an emoji gives, if it gives one: 👍 ✅ 🙌 yes, 👎 ❌ no, ❓ 🤔 👀 tell me more,
/// 🤷 your call, whatever skin tone, gender or presentation it's sent with.
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
        // A shrug, or a man's or woman's (🤷 then a joiner and ♂ or ♀).
        shrug if shrug.starts_with('🤷') => Some(Answer::Yours),
        _ => None,
    }
}

impl Answer {
    /// How it's said: `yes`, `no`, `tell me more` or `your call`.
    #[must_use]
    pub fn said(self) -> &'static str {
        match self {
            Self::Yes => "yes",
            Self::No => "no",
            Self::More => "tell me more",
            Self::Yours => "your call",
        }
    }

    /// The reaction an app answers with: 👍, 👎, ❓ or 🤷.
    #[must_use]
    pub fn reaction(self) -> &'static str {
        match self {
            Self::Yes => "👍",
            Self::No => "👎",
            Self::More => "❓",
            Self::Yours => "🤷",
        }
    }
}

impl Request {
    /// `session` asking `by` `question`, open, as `id` (`<machine>/<n>`): a yes does `yes`
    /// and a no does `no`. It recommends nothing and a yes can be undone, until
    /// [`Self::recommend`] and [`Self::irreversible`] are set.
    #[must_use]
    pub fn open(
        id: impl Into<String>,
        session: impl Into<String>,
        by: impl Into<String>,
        question: impl Into<String>,
        yes: impl Into<String>,
        no: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            question: question.into(),
            yes: yes.into(),
            no: no.into(),
            recommend: None,
            irreversible: false,
            session: session.into(),
            by: by.into(),
            state: State::Open,
        }
    }

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

    /// Whether the person may leave it to the session ([`Answer::Yours`]): never for a request
    /// that can't be undone, which a person decides.
    #[must_use]
    pub fn offers_yours(&self) -> bool {
        !self.irreversible
    }

    /// ` · 🤷 your call` where [`Self::offers_yours`], after the other answers.
    fn yours_offered(&self) -> &'static str {
        if self.offers_yours() {
            " · 🤷 your call"
        } else {
            ""
        }
    }

    /// What it says, for people and plain IRC clients.
    #[must_use]
    pub fn text(&self) -> String {
        match &self.state {
            State::Open => format!(
                "{}: {} 👍 yes: {} · 👎 no: {} · ❓ tell me more{}",
                self.by,
                self.question,
                self.yes,
                self.no,
                self.yours_offered()
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
            State::Withdrawn { why, .. } => format!(
                "{} {}, so needs no answer: {}",
                self.session,
                why.said(),
                self.question
            ),
        }
    }

    /// Whether it still waits on its person: open, a held yes they can still stop, or one
    /// they asked to know more of, which a yes or no still answers. Never one answered,
    /// taken as recommended or withdrawn.
    #[must_use]
    pub fn waits(&self) -> bool {
        match &self.state {
            State::Open | State::Held { .. } => true,
            State::Answered { answer, .. } => *answer == Answer::More,
            State::Withdrawn { .. } => false,
        }
    }

    /// What it asks, as two requests are compared ([`waiting`]): its question in lower case,
    /// its runs of spaces as one, without the punctuation or spaces at its end. Requests of
    /// one person that agree in this ask the same thing, and show once (the user's choice,
    /// 24 Nov 2026): never a guess that differently worded questions are one.
    #[must_use]
    pub fn subject(&self) -> String {
        let words: Vec<String> = self
            .question
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        words
            .join(" ")
            .trim_end_matches(|c: char| c.is_ascii_punctuation() || c == '？')
            .trim_end()
            .to_owned()
    }

    /// How it stands, as its card says: the card's state, and its note.
    fn standing(&self) -> (CardState, String) {
        match &self.state {
            State::Open => (
                CardState::Pending,
                format!(
                    "{}: react 👍 yes · 👎 no · ❓ tell me more{}{}",
                    self.by,
                    self.yours_offered(),
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
                answer: Answer::Yours,
                at,
                ..
            } => (
                CardState::Passed,
                format!(
                    "{} left it to {} at {}",
                    self.by,
                    self.session,
                    clock(at, false)
                ),
            ),
            State::Withdrawn { at, why } => (
                CardState::Cancelled,
                format!(
                    "{} {} at {}: no answer needed",
                    self.session,
                    why.said(),
                    clock(at, false)
                ),
            ),
            State::Answered {
                answer,
                at,
                recommended,
                ..
            } => (
                match answer {
                    Answer::Yes | Answer::Yours => CardState::Passed,
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
        }
    }

    /// The card apps draw for it: the question, yes and no (the recommended one first), and
    /// how to answer, or the answer.
    #[must_use]
    pub fn card(&self) -> Card {
        let yes = Field {
            label: "👍 yes".into(),
            value: crate::clip_line(&self.yes, 120),
        };
        let no = Field {
            label: "👎 no".into(),
            value: crate::clip_line(&self.no, 120),
        };
        let fields = match self.recommend {
            Some(Answer::No) => vec![no, yes],
            _ => vec![yes, no],
        };
        let (state, note) = self.standing();
        Card {
            state,
            about: Some(crate::clip_line(&self.session, 64)),
            fields,
            note: Some(crate::clip_line(&note, 120)),
            ..Card::new(self.id.clone(), crate::clip_line(&self.question, 80))
        }
    }
}

/// A request heard in [`CHANNEL`], as [`waiting`] takes them: only ones the server says its
/// person's own machine sent, which the caller checks, as nothing here can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posted {
    /// The server's id of the message that carried it.
    pub msgid: String,
    /// The request as that message said it.
    pub request: Request,
}

/// Something waiting for a person's answer: a request, with any others that ask the same
/// thing ([`Request::subject`]), which one answer answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waiting {
    /// The request, the first of them asked, as it stands now.
    pub request: Request,
    /// Its message, which a reaction on answers it.
    pub msgid: String,
    /// The others asking the same, in the order asked, each with its message: the answer
    /// goes on each.
    pub also: Vec<(Request, String)>,
}

impl Waiting {
    /// Every message an answer goes on: its own, then the others'.
    pub fn msgids(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.msgid.as_str()).chain(self.also.iter().map(|(_, id)| id.as_str()))
    }

    /// Whether the person may leave it to the sessions: only if every one asking offers it.
    #[must_use]
    pub fn offers_yours(&self) -> bool {
        self.request.offers_yours() && self.also.iter().all(|(other, _)| other.offers_yours())
    }
}

/// What waits for `by`'s answer among `posted`, the requests heard in [`CHANNEL`] oldest
/// first: each request that still [`Request::waits`], in the order first asked, those asking
/// the same thing as one.
///
/// A request is posted again whenever its state changes, under the same id: its first
/// posting is the message a reaction answers, and its last says where it stands. So one
/// answered, taken as recommended or withdrawn is gone from here as soon as its card says so,
/// in every app at once, and never before: an answer given shows only once the requesting
/// machine has taken it.
#[must_use]
pub fn waiting(posted: impl IntoIterator<Item = Posted>, by: &str) -> Vec<Waiting> {
    // By id, in the order first asked: the first posting's message, the last one's state.
    let mut asked: Vec<(String, Request)> = Vec::new();
    for Posted { msgid, request } in posted {
        if !request.by.eq_ignore_ascii_case(by) {
            continue;
        }
        match asked.iter_mut().find(|(_, held)| held.id == request.id) {
            Some((_, held)) => *held = request,
            None => asked.push((msgid, request)),
        }
    }
    let mut waiting: Vec<Waiting> = Vec::new();
    for (msgid, request) in asked {
        if !request.waits() {
            continue;
        }
        let subject = request.subject();
        match waiting
            .iter_mut()
            .find(|one| one.request.subject() == subject)
        {
            Some(one) => one.also.push((request, msgid)),
            None => waiting.push(Waiting {
                request,
                msgid,
                also: Vec::new(),
            }),
        }
    }
    waiting
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

/// What a row of the Office says of a session while one of its questions or requests waits
/// for the person (the user's choice, 28 Nov 2026: the question shows on its asker's row, not
/// pinned atop the Office): `Asked you · Land John's checkout fix?`.
#[must_use]
pub fn asked_you(question: &str) -> String {
    format!("{} · {question}", crate::home::ASKED_YOU)
}

/// When a question was asked, `seconds` ago, atop the opened question: `Asked you 25 min ago`.
#[must_use]
pub fn asked_you_ago(seconds: u64) -> String {
    format!("{} {}", crate::home::ASKED_YOU, crate::desk::ago(seconds))
}

/// What the opened question says when its session has kept working since asking: pushed
/// `pushes` times, or said a new task, `now_on` what it says it's on. An answer typed in the
/// agent's own tool never reaches zirc, so the person may have answered already; the session
/// withdraws the question once it has its answer (the user's choice, 28 Nov 2026).
///
/// `It has kept working since asking: 2 pushes, now on Rotate the staging certificates. If
/// you answered in the session, it'll take this back.`
#[must_use]
pub fn kept_working(pushes: usize, now_on: Option<&str>) -> String {
    let counted = match pushes {
        0 => None,
        1 => Some("1 push".to_owned()),
        n => Some(format!("{n} pushes")),
    };
    let parts: Vec<String> = [counted, now_on.map(|task| format!("now on {task}"))]
        .into_iter()
        .flatten()
        .collect();
    let since = if parts.is_empty() {
        "It has kept working since asking.".to_owned()
    } else {
        format!("It has kept working since asking: {}.", parts.join(", "))
    };
    format!("{since} If you answered in the session, it'll take this back.")
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_office_says_what_was_asked_when_and_whether_it_kept_working() {
        assert_eq!(
            super::asked_you("Land John's checkout fix?"),
            "Asked you · Land John's checkout fix?"
        );
        assert_eq!(super::asked_you_ago(25 * 60), "Asked you 25 min ago");
        assert_eq!(
            super::kept_working(2, Some("Rotate the staging certificates")),
            "It has kept working since asking: 2 pushes, now on Rotate the staging \
             certificates. If you answered in the session, it'll take this back."
        );
        assert_eq!(
            super::kept_working(1, None),
            "It has kept working since asking: 1 push. If you answered in the session, it'll \
             take this back."
        );
        assert_eq!(
            super::kept_working(0, None),
            "It has kept working since asking. If you answered in the session, it'll take \
             this back."
        );
    }

    use super::*;

    fn request() -> Request {
        Request {
            recommend: Some(Answer::Yes),
            ..Request::open(
                "mattias-mbp/1",
                "cc-1",
                "mattias",
                "Add a second CI node?",
                "Builds run in half the time; £40 a month more",
                "Builds stay as they are",
            )
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

    fn posted(msgid: &str, request: &Request) -> Posted {
        Posted {
            msgid: msgid.into(),
            request: request.clone(),
        }
    }

    fn answered(request: &Request, answer: Answer) -> Request {
        Request {
            state: State::Answered {
                answer,
                emoji: answer.reaction().into(),
                at: "2026-11-24T10:00:00Z".into(),
                recommended: false,
            },
            ..request.clone()
        }
    }

    #[test]
    fn what_waits_is_each_open_request_until_its_card_says_otherwise() {
        let node = request();
        let cert = Request::open("mattias-mbp/2", "cc-2", "mattias", "Revoke it?", "y", "n");
        let bobs = Request::open("bob-mbp/1", "cc-9", "bob", "Ship?", "y", "n");
        let heard = vec![
            posted("m1", &node),
            posted("m2", &cert),
            posted("m3", &bobs),
        ];
        let waits = waiting(heard.clone(), "Mattias");
        assert_eq!(
            waits.iter().map(|w| w.msgid.as_str()).collect::<Vec<_>>(),
            ["m1", "m2"],
            "his own, oldest first, never bob's"
        );
        // Answered: gone, as its card posted again says.
        let mut heard = heard;
        heard.push(posted("m4", &answered(&node, Answer::No)));
        let waits = waiting(heard.clone(), "mattias");
        assert_eq!(waits.len(), 1);
        assert_eq!(waits[0].request.id, "mattias-mbp/2");
        // Asked to know more: still waiting for a yes or no, on its first message.
        heard.push(posted("m5", &answered(&cert, Answer::More)));
        let waits = waiting(heard.clone(), "mattias");
        assert_eq!(waits[0].msgid, "m2");
        assert!(matches!(waits[0].request.state, State::Answered { .. }));
        // Withdrawn: gone.
        heard.push(posted(
            "m6",
            &Request {
                state: State::Withdrawn {
                    at: "2026-11-24T10:05:00Z".into(),
                    why: Withdrawal::Done,
                },
                ..cert.clone()
            },
        ));
        assert_eq!(waiting(heard, "mattias"), []);
    }

    #[test]
    fn requests_asking_the_same_thing_wait_as_one_which_one_answer_answers() {
        let agent = Request::open(
            "mattias-mbp/1",
            "cc-1",
            "mattias",
            "Merge the release branch now?",
            "y",
            "n",
        );
        let dispatcher = Request {
            irreversible: true,
            ..Request::open(
                "mattias-mbp/2",
                "dispatcher",
                "mattias",
                "  merge the  release branch NOW ?! ",
                "y",
                "n",
            )
        };
        let other = Request::open("mattias-mbp/3", "cc-1", "mattias", "Tag it too?", "y", "n");
        let heard = vec![
            posted("m1", &agent),
            posted("m2", &dispatcher),
            posted("m3", &other),
        ];
        let waits = waiting(heard.clone(), "mattias");
        assert_eq!(
            waits.len(),
            2,
            "the same session's other question is its own"
        );
        assert_eq!(waits[0].msgids().collect::<Vec<_>>(), ["m1", "m2"]);
        assert_eq!(waits[0].also[0].0.session, "dispatcher");
        assert!(!waits[0].offers_yours(), "one of them can't be undone");
        assert!(waits[1].offers_yours());
        // The first answered: the other still waits, alone.
        let mut heard = heard;
        heard.push(posted("m4", &answered(&agent, Answer::Yes)));
        let waits = waiting(heard, "mattias");
        assert_eq!(waits[0].msgid, "m2");
        assert!(waits[0].also.is_empty());
    }

    #[test]
    fn a_withdrawn_request_says_why_and_that_no_answer_is_needed() {
        let withdrawn = Request {
            state: State::Withdrawn {
                at: "2026-11-24T10:05:00Z".into(),
                why: Withdrawal::Gone,
            },
            ..request()
        };
        assert!(!withdrawn.waits());
        assert_eq!(withdrawn.card().state, CardState::Cancelled);
        assert_eq!(
            withdrawn.card().note.as_deref(),
            Some("cc-1 is gone at 10:05 UTC: no answer needed")
        );
        assert_eq!(
            withdrawn.text(),
            "cc-1 is gone, so needs no answer: Add a second CI node?"
        );
        assert_eq!(Request::of(&withdrawn.event()), Some(withdrawn));
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
