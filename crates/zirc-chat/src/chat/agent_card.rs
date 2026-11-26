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

use super::conversation::{EntryKind, Kind, fold};
use super::profile::ago_short;

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
    /// What it's called, leading, as its row and header say ([`super::Chat::agent_label`]):
    /// its title as given or as its tool names it, `Fix the coupon field's layout shift on
    /// phones`, at most two lines, else its task; or its handle when it has neither
    /// ([`Self::titled`]).
    pub title: String,
    /// Whether [`Self::title`] is other than its handle. When it isn't, the title is the
    /// handle, set in mono, and the card leaves out where it runs, what it's doing and when
    /// it was heard.
    pub titled: bool,
    /// Its short handle, `checkout`, in mono, or its id when it has none.
    pub handle: String,
    /// Whose agent it is: `Henrik Sandberg's agent`, `your agent`. `None` when its owner isn't
    /// known.
    pub whose: Option<String>,
    /// [`Self::whose`] as it follows the handle, muted, when the card is titled: ` · Henrik
    /// Chen's agent`, its separator first. Untitled, the title is the handle, and `whose`
    /// shows alone.
    pub after_handle: Option<String>,
    /// The tool it runs in, `claude-code`, for the tool's icon.
    pub tool: Option<String>,
    /// Where it runs, after the tool's icon: `Claude Code · mbp`.
    pub place: Option<String>,
    /// What it's doing now, from its latest intent or taking, marker first: `✎ about to
    /// change web/src/checkout/Coupon.tsx · 4m`, `⟶ the login bug · 12m`, or `Nothing in
    /// progress`.
    pub now: Option<String>,
    /// [`Self::now`] in parts, for apps that set its paths in mono and its age muted, drawn
    /// one after another as they are: its marker and words, `✎ about to change`, `⟶ the
    /// login bug`, or `Nothing in progress`.
    pub now_lead: Option<String>,
    /// The paths [`Self::now`] names after its lead, in mono: ` src/auth, src/login`, a
    /// space first.
    pub now_paths: Option<String>,
    /// How long ago it said so, muted, last: ` · 4m`, its separator first.
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
        // Its title first, its handle under it, as its row and header (the user's choice, 25
        // Nov 2026).
        let title = self.agent_label(nick, session);
        let handle = self.session_label(session);
        let titled = self.agent_handle(session, &title).is_some();
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
        // When it was heard and whether it's online, titled or not, as its header says.
        let doing = self.doing_now(nick, session, now);
        let (heard, active) = self.heard_line(nick, session, doing.active, now);
        let registered = self.session_titles.get(session).map(|(_, card)| card);
        if registered.is_none() && !titled {
            return AgentCard {
                session: session.to_owned(),
                title,
                titled: false,
                handle,
                whose,
                after_handle: None,
                tool: None,
                place: None,
                now: None,
                now_lead: None,
                now_paths: None,
                now_ago: None,
                task: None,
                heard: Some(heard),
                active,
                mine,
                message_label: MESSAGE.to_owned(),
            };
        }
        let tool = registered
            .and_then(|card| card.tool.clone())
            .or_else(|| self.session_tools.get(session).cloned());
        let place: Vec<String> = tool
            .as_deref()
            .map(zirc_proto::tool_name)
            .into_iter()
            .chain(whose_parts.map(|(_, machine)| machine))
            .collect();
        AgentCard {
            session: session.to_owned(),
            title,
            titled,
            handle,
            after_handle: whose
                .as_ref()
                .filter(|_| titled)
                .map(|whose| format!("{SEPARATOR}{whose}")),
            whose,
            tool,
            place: (!place.is_empty()).then(|| place.join(SEPARATOR)),
            now: Some(doing.line()),
            now_lead: Some(doing.lead.clone()),
            now_paths: doing.paths_text(),
            now_ago: doing.ago_text(),
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

    /// [`Chat::heard`] for every session at once, in one pass over what's held rather than
    /// one a session, for a list of them.
    pub(super) fn heard_all(&self) -> Heard {
        let mut heard = Heard::new();
        for entry in self.conversations.iter().flat_map(|c| &c.entries) {
            if let EntryKind::Zirc(message) = &entry.kind {
                let at = heard
                    .entry((fold(&entry.from), message.session.clone()))
                    .or_insert(entry.time);
                *at = (*at).max(entry.time);
            }
        }
        heard
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

/// When each agent session was last heard, by its machine's nick, folded, and its id
/// ([`Chat::heard_all`]).
pub(super) type Heard = std::collections::HashMap<(String, String), DateTime<Utc>>;

/// Between the parts of a card's line: `Claude Code · mbp`, `checkout · Henrik Sandberg's agent`.
pub(super) const SEPARATOR: &str = " · ";

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
        format!(
            "{}{}{}",
            self.lead,
            self.paths_text().unwrap_or_default(),
            self.ago_text().unwrap_or_default()
        )
    }

    /// The paths as they follow the lead: ` src/auth, src/login`.
    fn paths_text(&self) -> Option<String> {
        (!self.paths.is_empty()).then(|| format!(" {}", self.paths.join(", ")))
    }

    /// How long ago, as it follows the rest: ` · 8m`.
    fn ago_text(&self) -> Option<String> {
        self.ago.as_ref().map(|ago| format!("{SEPARATOR}{ago}"))
    }
}
