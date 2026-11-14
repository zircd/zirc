//! An agent session's card, as every app opens it from the session's name or face: its full
//! title, whose agent it is, where it runs, what it's doing now and when it was last heard
//! (the design lead's STEP-SESSION-CARD spec; the user's decision that an agent's visible
//! name stays its short handle, and the full picture lives here). Every word on it comes
//! from here; apps only draw it.
//!
//! "Open session" is the one thing not decided here: the link to open a session is its
//! owner's alone, kept on their machine and served over its zircd's socket. The card says
//! whether the session is [`AgentCard::mine`]; the desktop app asks its zircd for the link
//! and draws the button only when it gets one.

use chrono::{DateTime, Utc};
use zirc_proto::Event;
use zirc_proto::session::SessionCard;

use super::conversation::{EntryKind, Kind};
use super::profile::{Standing, ago_short};

/// What the Message button says.
pub const MESSAGE: &str = "Message";

/// What the button that opens the session in its tool says, on the owner's desktop app.
pub const OPEN_SESSION: &str = "Open session";

/// What an agent session's card shows, top to bottom ([`super::Chat::agent_card`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct AgentCard {
    /// The session's id, `cc-4e21`, in mono and muted, for copying.
    pub session: String,
    /// Its full title as its tool names it, `Fix the coupon field's layout shift on
    /// phones`, at most two lines; or its handle when it hasn't said one ([`Self::titled`]).
    pub title: String,
    /// Whether [`Self::title`] is the title it registered with `zirc name --title`. When
    /// it isn't, the title is the handle, set in mono, and the card leaves out where it
    /// runs, what it's doing and when it was heard.
    pub titled: bool,
    /// Its short handle, `checkout`, in mono, or its id when it has none.
    pub handle: String,
    /// Whose agent it is, after the handle: `Maya Chen's agent`, `your agent`. `None` when
    /// its owner isn't known.
    pub whose: Option<String>,
    /// The tool it runs in, `claude-code`, for the tool's icon.
    pub tool: Option<String>,
    /// Where it runs, after the tool's icon: `Claude Code · mbp`.
    pub place: Option<String>,
    /// Its owner's away, on its own line: `Maya Chen is away: lunch`.
    pub standing: Option<String>,
    /// What it's doing now, from its latest intent or taking, marker first: `✎ about to
    /// change web/src/checkout/Coupon.tsx · 4m`, `⟶ the login bug · 12m`, or `Nothing in
    /// progress`.
    pub now: Option<String>,
    /// [`Self::now`] in parts, for apps that set its paths in mono: its marker and words,
    /// `✎ about to change`, `⟶ the login bug`, or `Nothing in progress`.
    pub now_lead: Option<String>,
    /// The paths [`Self::now`] names after its lead, in mono, comma-separated.
    pub now_paths: Vec<String>,
    /// How long ago it said so, after a `·`: `4m`.
    pub now_ago: Option<String>,
    /// The task under [`Self::now`], when an intent says one: `refactor login`.
    pub task: Option<String>,
    /// When it was last heard, in a caption: `Last heard 3m ago`, or `Active now` with the
    /// here dot while [`Self::active`].
    pub heard: Option<String>,
    /// Whether it's at work right now: taking something on, or about to change something.
    pub active: bool,
    /// Whether it's the viewer's own agent: only then may the desktop app, on the machine
    /// it runs on, offer [`OPEN_SESSION`].
    pub mine: bool,
    /// What its Message button says: [`MESSAGE`], so apps hold no wording.
    pub message_label: String,
}

impl super::Chat {
    /// The card of the agent session `session` on the machine `nick`, at `now`: see
    /// [`AgentCard`].
    #[must_use]
    pub fn agent_card(&self, nick: &str, session: &str, now: DateTime<Utc>) -> AgentCard {
        let handle = self.session_label(session);
        let whose_parts = self.whose(nick);
        let mine = whose_parts
            .as_ref()
            .is_some_and(|(owner, _)| owner.eq_ignore_ascii_case(self.me()));
        let owner = whose_parts
            .as_ref()
            .map(|(owner, _)| self.owner_name(owner));
        let whose = owner
            .as_deref()
            .map(|owner| super::agent_name(None, owner).0);
        let standing = match self.profile(nick).away {
            Some(why) => {
                let away = Standing::Away(why).label();
                Some(match whose_parts.as_ref() {
                    Some((owner, _)) if !mine => format!("{} is {away}", self.shown_name(owner)),
                    _ if mine => format!("You're {away}"),
                    _ => away,
                })
            }
            None => None,
        };
        let titled = self.session_titles.get(session).map(|(_, card)| card);
        let Some(registered) = titled else {
            return AgentCard {
                session: session.to_owned(),
                title: handle.clone(),
                titled: false,
                handle,
                whose,
                tool: None,
                place: None,
                standing,
                now: None,
                now_lead: None,
                now_paths: Vec::new(),
                now_ago: None,
                task: None,
                heard: None,
                active: false,
                mine,
                message_label: MESSAGE.to_owned(),
            };
        };
        let tool = registered
            .tool
            .clone()
            .or_else(|| self.session_tools.get(session).cloned());
        let place: Vec<String> = tool
            .as_deref()
            .map(super::profile::tool_name)
            .into_iter()
            .chain(whose_parts.map(|(_, machine)| machine))
            .collect();
        let doing = self.doing_now(nick, session, now);
        let (heard, active) = self.heard_line(nick, session, doing.active, now);
        AgentCard {
            session: session.to_owned(),
            title: registered.title.clone(),
            titled: true,
            handle,
            whose,
            tool,
            place: (!place.is_empty()).then(|| place.join(" · ")),
            standing,
            now: Some(doing.line()),
            now_lead: Some(doing.lead),
            now_paths: doing.paths,
            now_ago: doing.ago,
            task: doing.task,
            heard: Some(heard),
            active,
            mine,
            message_label: MESSAGE.to_owned(),
        }
    }

    /// The title `card` registers for its session, when it's newer than the one known: as
    /// its machine said it at `at`.
    pub(super) fn learn_title(&mut self, card: SessionCard, at: DateTime<Utc>) {
        let newer = self
            .session_titles
            .get(&card.session)
            .is_none_or(|(known, _)| *known <= at);
        if newer {
            self.session_titles.insert(card.session.clone(), (at, card));
        }
    }

    /// What the session is doing at `now`: its live intent, else what it's taking on, with
    /// how long ago it said so; `Nothing in progress` when neither.
    pub(super) fn doing_now(&self, nick: &str, session: &str, now: DateTime<Utc>) -> Doing {
        let seconds = u64::try_from(now.timestamp()).unwrap_or_default();
        let ours = |n: &str, s: &str| n.eq_ignore_ascii_case(nick) && s == session;
        let intent = self
            .conversations
            .iter()
            .filter(|c| c.kind == Kind::Channel)
            .flat_map(|c| self.presence.intents(&c.name, seconds))
            .find(|intent| ours(&intent.nick, &intent.session));
        let ago = |at: DateTime<Utc>| ago_short(at, now);
        if let Some(intent) = intent {
            let said = self.said_last(nick, session, |event| matches!(event, Event::Intent { .. }));
            let glyph = super::Icon::Intent.glyph();
            let summary = super::strip_formatting(&intent.summary);
            let (lead, task) = if intent.paths.is_empty() {
                (format!("{glyph} {summary}"), None)
            } else {
                (
                    format!("{glyph} about to change"),
                    Some(summary).filter(|task| !task.is_empty()),
                )
            };
            return Doing {
                lead,
                paths: intent.paths,
                ago: said.map(ago),
                task,
                active: true,
            };
        }
        if let Some((task, at)) = self.claim(nick, session) {
            return Doing {
                lead: format!("{} {task}", super::Icon::Taking.glyph()),
                paths: Vec::new(),
                ago: Some(ago(at)),
                task: None,
                active: true,
            };
        }
        Doing {
            lead: "Nothing in progress".to_owned(),
            paths: Vec::new(),
            ago: None,
            task: None,
            active: false,
        }
    }

    /// When the session last said anything, anywhere we are.
    pub(super) fn heard(&self, nick: &str, session: &str) -> Option<DateTime<Utc>> {
        self.said_last(nick, session, |_| true)
    }

    /// When the session `session` on `nick` last said an event that's `wanted`, anywhere
    /// we are.
    fn said_last(
        &self,
        nick: &str,
        session: &str,
        wanted: impl Fn(&Event) -> bool,
    ) -> Option<DateTime<Utc>> {
        self.conversations
            .iter()
            .filter_map(|conversation| {
                conversation
                    .entries
                    .iter()
                    .rev()
                    .find_map(|entry| match &entry.kind {
                        EntryKind::Zirc(message)
                            if message.session == session
                                && entry.from.eq_ignore_ascii_case(nick)
                                && wanted(&message.event) =>
                        {
                            Some(entry.time)
                        }
                        _ => None,
                    })
            })
            .max()
    }
}

/// What a session is doing, in the parts its card's Now line draws.
pub(super) struct Doing {
    /// Its marker and words: `✎ about to change`, `⟶ the login bug`.
    pub(super) lead: String,
    /// The paths after them, in mono.
    pub(super) paths: Vec<String>,
    /// How long ago it said so: `4m`.
    pub(super) ago: Option<String>,
    /// An intent's task, under the line.
    pub(super) task: Option<String>,
    /// Whether it's at work.
    pub(super) active: bool,
}

impl Doing {
    /// The line as one string, as the terminal client prints it: `✎ about to change
    /// src/auth · 8m`.
    pub(super) fn line(&self) -> String {
        let mut line = self.lead.clone();
        if !self.paths.is_empty() {
            line.push(' ');
            line.push_str(&self.paths.join(", "));
        }
        if let Some(ago) = &self.ago {
            line.push_str(" · ");
            line.push_str(ago);
        }
        line
    }
}
