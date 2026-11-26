//! An agent session's conversation, as every app opens it from its direct message: what
//! the session did, in every room we're in, together with what we and it said to each
//! other. The session's lines go to the repository's agents room and the landings to its
//! channel, while the direct conversation `<machine>/<session>` holds only what was said
//! between us, so without this it opens empty.
//!
//! A message carries the session that sent it ([`zirc_proto::Message::session`]) and arrives
//! from its machine's nick, so `(nick, session)` finds a session's every line in any channel.
//! CI's landings and deploys come from CI's nick and only *name* the session, in their data
//! ([`zirc_proto::ci::Data::session`]).

use std::collections::HashSet;

use zirc_proto::{Event, ci};

use super::conversation::{Conversation, Entry, EntryKind, Kind, agent};
use super::{Avatar, Chat, SenderKind};

/// The two letters on a machine's face where it speaks itself: a glyph, never the person's
/// photo.
pub const AGENTS_GLYPH: &str = "AG";

/// The label of the machine thread's [`Avatar::App`], for apps that draw it their own way.
pub const AGENTS_LABEL: &str = "agents";

/// One line of an agent's conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityEntry {
    /// The entry, as the room or direct conversation holds it. A copy: reactions and the
    /// unread counts stay with where it was said.
    pub entry: Entry,
    /// The room it was said in, `#web/agents`, for the muted chip after its time; `None` for
    /// what we and the agent said to each other, which has no chip. The entry's id says
    /// which message to open the room at.
    pub room: Option<String>,
}

/// What stands in the place of the composer where the person can't write: an agent that
/// isn't ours, whose machine hears only its owner, one of ours that has ended, and zirc's
/// own direct messages ([`Chat::read_only_note`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ReadOnlyNote {
    /// The line: `Only Henrik Sandberg can message checkout`, or `checkout ended 6 days ago on
    /// your mbp. Messages can't reach it`.
    pub text: String,
    /// The button: `Message Henrik Sandberg`, or `Message your mbp's agents`. Empty, as is
    /// [`Self::dm`], where there's no one to go to instead, as in zirc's own direct
    /// messages: apps then show no button.
    pub button: String,
    /// The conversation the button opens: the owner's nick, or the machine's own thread,
    /// where a line reaches its agents working now.
    pub dm: String,
    /// A second button, for an agent of ours that has ended: [`REMOVE_FROM_MESSAGES`],
    /// which closes the conversation ([`Chat::close_direct`]), its history kept for Jump to
    /// and search. Empty for none.
    pub remove: String,
}

/// What an ended agent's note offers besides: takes its conversation out of Messages.
pub const REMOVE_FROM_MESSAGES: &str = "Remove from Messages";

/// The top of an agent's conversation, before its first entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentIntro {
    /// The card's title: its full title, or its handle when it hasn't registered one.
    pub title: String,
    /// Where it runs, `Claude Code · mbp`, wherever its tool or machine is known, whether or
    /// not it registered a title.
    pub place: Option<String>,
    /// Its id, `cc-4e21`, in mono.
    pub session: String,
}

/// What an agent conversation's header says under its name ([`Chat::agent_header`]): the
/// card's Now line, for every session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentHeader {
    /// What it's doing, marker first: `✎ about to change web/src · 4m`, or `Nothing in
    /// progress`.
    pub now: String,
    /// [`Self::now`] in parts, for apps that set its paths in mono: its marker and words.
    pub lead: String,
    /// The paths after the lead, in mono.
    pub paths: Vec<String>,
    /// How long ago it said so: `4m`.
    pub ago: Option<String>,
    /// The task under the line, when an intent says one.
    pub task: Option<String>,
    /// `Active now`, or `Last heard 3m ago`, or `Not heard yet`.
    pub heard: String,
    /// Whether it's at work right now.
    pub active: bool,
    /// The one line every app's header says under its handle, after the here dot while
    /// [`Self::active`]: what it's doing while it's at it ([`Self::now`]), unless it isn't
    /// listening with messages waiting, else [`Self::heard`]; a teammate's agent's says whose it is first, `Henrik Sandberg's agent ·
    /// Last heard 3m ago`, where ours needs no `Your agent`: its AGENT tag says it.
    pub line: String,
}

impl Chat {
    /// The conversation with the agent session `name` (`<machine>/<session>`), in time
    /// order: its activity in every room we're in, and the direct messages between us and
    /// it. Empty for anything that isn't an agent conversation.
    ///
    /// Its activity is what it said (intents, `taking:`, `done:` and `dropped:` posts, any
    /// other text, commits, pushes, decisions, files) and the rows that name it for files
    /// both changing; and the landings and deploys whose data names it. Its working sets are
    /// not entries and never here. Bounded by what's loaded.
    ///
    /// Reading this changes nothing: activity counts no unread here, it did or didn't in its
    /// room, and only the session's direct messages to us count, as they always did.
    #[must_use]
    pub fn agent_activity(&self, name: &str) -> Vec<ActivityEntry> {
        let Some((nick, session)) = agent(name) else {
            return Vec::new();
        };
        let mut found: Vec<ActivityEntry> = Vec::new();
        if let Some(direct) = self.conversation(name) {
            found.extend(
                direct
                    .entries
                    .iter()
                    .filter(|entry| !entry.is_session_card())
                    .map(|entry| ActivityEntry {
                        entry: entry.clone(),
                        room: None,
                    }),
            );
        }
        for room in self
            .conversations
            .iter()
            .filter(|c| c.kind == Kind::Channel && c.joined)
        {
            found.extend(
                room.entries
                    .iter()
                    // A wake a room's message caused is that room's alone.
                    .filter(|entry| {
                        is_activity_of(nick, session, entry) && !entry.is_woke_in(&room.name)
                    })
                    .map(|entry| ActivityEntry {
                        entry: entry.clone(),
                        room: Some(room.name.clone()),
                    }),
            );
        }
        // Oldest first, the order kept for those said at the same moment; the same message
        // seen twice (history and live) once.
        found.sort_by_key(|activity| activity.entry.time);
        let mut seen = HashSet::new();
        found.retain(|activity| {
            activity
                .entry
                .id
                .as_ref()
                .is_none_or(|id| seen.insert(id.clone()))
        });
        found
    }

    /// Whether the person can message the conversation called `name`: not an agent session
    /// of someone else's, whose machine hears only its owner, nor zirc's system account,
    /// which reads no replies ([`Chat::read_only_note`]). Everything else, ours and every
    /// other conversation, can.
    #[must_use]
    pub fn can_message(&self, name: &str) -> bool {
        !super::is_system(name)
            && agent(name).is_none_or(|(nick, session)| {
                self.ours(nick) && self.session_live(nick, session) != Some(false)
            })
    }

    /// What replaces the composer where the person can't write: in the conversation with a
    /// teammate's agent, `Only Henrik Sandberg can message checkout`, with a `Message Henrik Sandberg`
    /// button that opens his direct conversation; in zirc's own direct messages,
    /// `Zirc's notices can't be answered.` and no button; in the conversation with one of
    /// our agents that has ended, `checkout ended 6 days ago on your mbp. Messages can't
    /// reach it`, with `Message your mbp's agents`, which opens the machine's own thread, and `Remove
    /// from Messages` (the user's choices, 25 Nov 2026). `None` where [`Chat::can_message`].
    #[must_use]
    pub fn read_only_note(&self, name: &str) -> Option<ReadOnlyNote> {
        if super::is_system(name) {
            return Some(ReadOnlyNote {
                text: format!("{}'s notices can't be answered.", zirc_proto::NAME),
                button: String::new(),
                dm: String::new(),
                remove: String::new(),
            });
        }
        let (nick, session) = agent(name)?;
        if self.ours(nick) {
            return (self.session_live(nick, session) == Some(false))
                .then(|| self.ended_note(nick, session));
        }
        // A machine whose host says no owner, as one not logged in through zirc.dev's: it
        // hears no one's messages, ours included, until it's logged in.
        let Some((owner, _)) = self.machine(nick) else {
            return Some(ReadOnlyNote {
                text: format!(
                    "{nick} doesn't say whose it is, so it can't hear you: run `zirc login` on it"
                ),
                button: String::new(),
                dm: String::new(),
                remove: String::new(),
            });
        };
        let who = self.owner_name(&owner);
        Some(ReadOnlyNote {
            text: format!("Only {who} can message {}", self.session_label(session)),
            button: format!("Message {who}"),
            dm: owner,
            remove: String::new(),
        })
    }

    /// The note in place of the composer of our agent `session` on the machine `nick`, which
    /// has ended: when, as far as anyone knows, by when it last listened or was last heard.
    fn ended_note(&self, nick: &str, session: &str) -> ReadOnlyNote {
        let who = self.session_label(session);
        let ended = self
            .listened_at(nick, session)
            .or_else(|| self.heard(nick, session))
            .map(|at| {
                let ago = u64::try_from(((self.clock)() - at).num_seconds()).unwrap_or(0);
                super::profile::span_words(ago)
            });
        let machine = self.machine_said(nick);
        let text = match ended {
            Some(ago) => format!("{who} ended {ago} ago on {machine}. Messages can't reach it"),
            None => format!("{who} ended on {machine}. Messages can't reach it"),
        };
        ReadOnlyNote {
            text,
            button: format!("Message {machine}'s agents"),
            dm: nick.to_owned(),
            remove: REMOVE_FROM_MESSAGES.to_owned(),
        }
    }

    /// The top of the conversation with the agent session `name`: its card's title, where
    /// it runs, and its id. `None` for any other conversation.
    #[must_use]
    pub fn agent_intro(&self, name: &str) -> Option<AgentIntro> {
        let (nick, session) = agent(name)?;
        let card = self.agent_card(nick, session, (self.clock)());
        // Where it runs, for every session where it's known, titled or not: its tool from
        // what it said, and its machine.
        let place: Vec<String> = card
            .tool
            .or_else(|| self.session_tools.get(session).cloned())
            .as_deref()
            .map(zirc_proto::tool_name)
            .into_iter()
            .chain(self.whose(nick).map(|(_, machine)| machine))
            .collect();
        Some(AgentIntro {
            title: card.title,
            place: (!place.is_empty()).then(|| place.join(" · ")),
            session: card.session,
        })
    }

    /// What the header of the conversation with the agent session `name` says under its
    /// name, for every session whether or not it registered a title: what it's doing now
    /// (marker and words, as [`AgentCard::now`]) and when it was heard, `Active now` or
    /// `Last heard 3m ago`, and the one line of them every app draws
    /// ([`AgentHeader::line`]). `None` for any other conversation.
    #[must_use]
    pub fn agent_header(&self, name: &str) -> Option<AgentHeader> {
        let (nick, session) = agent(name)?;
        let now = (self.clock)();
        let doing = self.doing_now(nick, session, now);
        let (heard, active) = self.heard_line(nick, session, doing.active, now);
        // Not listening with messages waiting says so before what it was doing.
        let waiting = self.not_listening(nick, session).is_some();
        let line = if doing.active && !waiting {
            doing.line()
        } else {
            match self.whose(nick) {
                Some((owner, _)) if !owner.eq_ignore_ascii_case(self.me()) => {
                    let whose = super::agent_name(None, &self.owner_name(&owner)).0;
                    format!("{whose} · {heard}")
                }
                _ => heard.clone(),
            }
        };
        Some(AgentHeader {
            line,
            now: doing.line(),
            lead: doing.lead,
            paths: doing.paths,
            ago: doing.ago,
            task: doing.task,
            heard,
            active,
        })
    }

    /// What the conversation with the agent session `name` says while it has nothing to
    /// show: `Nothing from checkout yet. What it says, changes and lands shows here.`
    /// `None` once [`Chat::agent_activity`] has anything, and for other conversations.
    #[must_use]
    pub fn agent_empty(&self, name: &str) -> Option<String> {
        let (_, session) = agent(name)?;
        self.agent_activity(name).is_empty().then(|| {
            format!(
                "Nothing from {} yet. What it says, changes and lands shows here.",
                self.session_label(session)
            )
        })
    }

    /// The face of a machine's thread: the agents' glyph, never the person's photo.
    #[must_use]
    pub fn agents_avatar() -> Avatar {
        Avatar::App {
            glyph: AGENTS_GLYPH.to_owned(),
            label: AGENTS_LABEL.to_owned(),
        }
    }
}

/// `activity` as a conversation named `name`, for the code that lays out and draws a
/// conversation's lines ([`Chat::items`]) to draw an agent's. Its entries keep the order
/// given, and rooms are looked up by their index in `activity`.
#[must_use]
pub fn activity_conversation(name: &str, activity: &[ActivityEntry]) -> Conversation {
    let mut conversation = Conversation::new(name, Kind::Query);
    conversation.entries = activity.iter().map(|a| a.entry.clone()).collect();
    conversation
}

/// Whether `entry`, said in a room, is activity of the session `session` of the machine
/// `nick`.
fn is_activity_of(nick: &str, session: &str, entry: &Entry) -> bool {
    let EntryKind::Zirc(message) = &entry.kind else {
        return false;
    };
    if entry.is_session_card() || matches!(message.event, Event::Working { .. }) {
        return false;
    }
    if entry.from.eq_ignore_ascii_case(nick) && message.session == session {
        return true;
    }
    // An overlap it settled is its own whichever side said so.
    if let Some(settled) = zirc_proto::woke::Settled::of(&message.event) {
        return settled.first.is(nick, session) || settled.second.is(nick, session);
    }
    // CI says a landing or deploy from its own nick, and names the session that queued it,
    // and its machine: a session's id is unique only on its machine. An older CI's event
    // names no machine, and goes by the session alone.
    match &message.event {
        Event::Custom { kind, data, .. }
            if (kind == ci::LANDING_KIND || kind == ci::DEPLOY_KIND)
                && super::sender_kind(&entry.from, entry.owner.as_deref())
                    == SenderKind::Integration =>
        {
            let data = ci::Data::of(data);
            data.session.as_deref() == Some(session)
                && data
                    .machine
                    .as_deref()
                    .is_none_or(|machine| machine.eq_ignore_ascii_case(nick))
        }
        _ => false,
    }
}
