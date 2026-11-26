//! One conversation: a channel, a private one with a nick, or the server's own messages.
//! What was said in it, who is in it, and how much of it hasn't been read yet.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use zirc_proto::{Level, Message};

/// What a conversation is with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// The server's, shown before any channel is: only what the client says while it's
    /// shown. What the server and its services say goes in [`super::Chat::status`].
    Server,
    /// A channel.
    Channel,
    /// Private messages with one nick.
    Query,
}

/// Status prefixes, highest first, as `NAMES` replies give them.
pub const PREFIXES: &str = "~&@%+";

/// Someone in a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// Their nick.
    pub nick: String,
    /// Their highest status in the channel, such as `@` for an operator.
    pub prefix: Option<char>,
}

/// One emoji given to a message, and by whom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reaction {
    /// The emoji.
    pub emoji: String,
    /// Who gave it, in order.
    pub nicks: Vec<String>,
}

impl Reaction {
    /// Whether `nick` gave it.
    #[must_use]
    pub fn by(&self, nick: &str) -> bool {
        self.nicks.iter().any(|n| n.eq_ignore_ascii_case(nick))
    }
}

/// How many reactions given a conversation keeps the times of, for Activity.
const GIVEN_KEPT: usize = 500;

/// A reaction given, and when: what Activity lists of reactions to our messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Given {
    /// The message it was given to, by the server's id.
    pub id: String,
    /// Who gave it.
    pub nick: String,
    /// The emoji.
    pub emoji: String,
    /// When.
    pub at: DateTime<Utc>,
}

/// Something said or done in a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// When the server says it happened, or when it arrived.
    pub time: DateTime<Utc>,
    /// The server's id for it (`IRCv3`'s `msgid`), which history repeats and reactions name.
    pub id: Option<String>,
    /// The nick it's from: the speaker, who came or went, the old nick of a rename, or who
    /// removed someone. Empty for what the client itself says.
    pub from: String,
    /// Whose machine it's from, its owner's handle, as the sender's host said
    /// (`machine/<owner>`, which only the server sets): None from a person.
    pub owner: Option<String>,
    /// What happened.
    pub kind: EntryKind,
    /// Whether it came from the server's history rather than live.
    pub history: bool,
    /// Whether it mentions us. Every message in a private conversation does.
    pub mention: bool,
    /// The zirc app a message was sent from, such as `desktop`, from its
    /// [`CLIENT_TAG`]. `None` from plain IRC clients.
    pub sent_from: Option<String>,
    /// The message it answers, by the server's id, when it's a reply (`IRCv3`'s
    /// `+draft/reply` on a message). Plain IRC clients show a reply as an ordinary line.
    pub reply_to: Option<String>,
    /// A message's, action's or notice's text with IRC's bold, colour and similar codes
    /// kept, when it has any, for [`Entry::rich`] to draw them. Everything else reads the
    /// text in [`Entry::kind`], which has them taken out.
    pub formatted: Option<String>,
    /// Whether its author corrected it since ([`super::Chat::say_edit`]): what it says now
    /// is the correction's.
    pub edited: bool,
    /// Its sealed parts, each a `[sealed]` in its text, when it has some
    /// ([`super::Chat::sealed_parts`]).
    pub sealed: Option<super::SealedTag>,
    /// The files attached to a message, which its text no longer names
    /// ([`zirc_proto::attached`]): drawn under its words. None on anything else.
    pub files: Vec<zirc_proto::File>,
}

/// What a reply answers, as apps show it above the reply, as `↪ bob: first line…`, and jump
/// to by its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quote {
    /// The answered message's id.
    pub id: String,
    /// Who said it: their nick.
    pub from: String,
    /// Who said it, as apps name them: an agent session by its name, the machine itself as
    /// its owner's agent, an app by its label, and a person as they show
    /// ([`super::sender_text`]). Never a machine's nick.
    pub name: String,
    /// Its first line, cut to [`QUOTE_CHARS`] characters, ending … when cut.
    pub text: String,
    /// Who said it, as their row's avatar shows them ([`Chat::speaker`]).
    pub avatar: super::Avatar,
}

/// The most of a message's first line a [`Quote`] keeps.
pub const QUOTE_CHARS: usize = 80;

/// The client-only tag that says which zirc app sent a message. Ergo passes it on and
/// keeps it in history.
pub const CLIENT_TAG: &str = "+zirc.dev/client";

impl Entry {
    /// The repository it's about, as a zirc message says ([`zirc_proto::Message::repo`]).
    #[must_use]
    pub fn repo(&self) -> Option<&str> {
        match &self.kind {
            EntryKind::Zirc(message) => message.repo.as_deref(),
            _ => None,
        }
    }

    /// Where it counts as said, in `conversation`: in a room every repository shares, its
    /// repository's scope ([`crate::names::scope_of`]), whose forge and checkout its files
    /// and commits link to; else `conversation`.
    #[must_use]
    pub fn place(&self, conversation: &str) -> String {
        crate::names::scope_of(conversation, self.repo())
    }

    /// The agent session it came from, for a zirc message ([`super::agent_session`]).
    #[must_use]
    pub fn agent_session(&self) -> Option<&str> {
        match &self.kind {
            EntryKind::Zirc(message) => super::agent_session(&self.from, message),
            _ => None,
        }
    }

    /// Whether it's an integration coming or going, joining, leaving, quitting, removed or
    /// renamed: never a row, as one connects only for the moment it says something, such as
    /// CI's posts. What it says, and its reactions, are rows as anyone's.
    #[must_use]
    pub fn is_integration_coming_or_going(&self) -> bool {
        match &self.kind {
            EntryKind::Join | EntryKind::Part(_) | EntryKind::Quit(_) => is_integration(&self.from),
            EntryKind::Kick { nick, .. } => is_integration(nick),
            EntryKind::Nick(new) => is_integration(&self.from) || is_integration(new),
            _ => false,
        }
    }

    /// Something the client says, now.
    #[must_use]
    pub fn info(time: DateTime<Utc>, text: String) -> Self {
        Self {
            time,
            id: None,
            from: String::new(),
            owner: None,
            kind: EntryKind::Info(text),
            history: false,
            mention: false,
            sent_from: None,
            reply_to: None,
            formatted: None,
            edited: false,
            sealed: None,
            files: Vec::new(),
        }
    }

    /// Whether it is something said: a message, an action, a notice or a zirc message, but
    /// not a session's title card, which is no row of a conversation ([`Entry::is_session_card`]).
    /// Joins, parts, topics and the client's own words are not.
    #[must_use]
    pub fn is_said(&self) -> bool {
        matches!(
            self.kind,
            EntryKind::Message(_)
                | EntryKind::Action(_)
                | EntryKind::Notice(_)
                | EntryKind::Zirc(_)
                | EntryKind::Unreadable(_)
        ) && !self.is_session_card()
    }

    /// Whether a list row shows it: what [`super::Chat::preview`] previews and, as the one
    /// rule, what orders every list by recency ([`Conversation::last_said`]), so a row never
    /// sorts by a time it doesn't show. Something someone said, a message, action, notice or
    /// zirc post, but never a card's live update, which only redraws its card: a running
    /// landing's steps come every few seconds; nor a wake's line ([`Entry::is_woke`]), which
    /// is for whoever opens the agent's conversation.
    #[must_use]
    pub fn is_shown_in_list(&self) -> bool {
        !self.from.is_empty()
            && !self.is_session_card()
            && self.live_card().is_none()
            && !self.is_woke()
            && matches!(
                self.kind,
                EntryKind::Message(_)
                    | EntryKind::Action(_)
                    | EntryKind::Notice(_)
                    | EntryKind::Zirc(_)
            )
    }

    /// Whether it counts towards what's unread when `me` hasn't read it: someone else
    /// saying something, but never a wake's line, wherever it's shown.
    #[must_use]
    pub fn counts(&self, me: &str) -> bool {
        self.is_said()
            && !self.is_woke()
            && !self.from.is_empty()
            && !self.from.eq_ignore_ascii_case(me)
    }

    /// What it asks of whoever reads it: a zirc message's level ([`zirc_proto::Message::level`]),
    /// info for anything else.
    #[must_use]
    pub fn level(&self) -> Level {
        match &self.kind {
            EntryKind::Zirc(message) => message.level_or_info(),
            _ => Level::Info,
        }
    }

    /// Whether it's a commit or a push zircd said at info: sessions read them to find
    /// overlaps, and a person follows a task by its `taking:` and `done:`, and its landing.
    fn is_commit_or_push(&self) -> bool {
        matches!(&self.kind, EntryKind::Zirc(message)
            if matches!(message.event, zirc_proto::Event::Commit { .. } | zirc_proto::Event::Push { .. })
                && message.level_or_info() == Level::Info)
    }

    /// Whether it's a wake's line ([`zirc_proto::woke`]), never unread anywhere.
    #[must_use]
    pub fn is_woke(&self) -> bool {
        matches!(&self.kind, EntryKind::Zirc(message) if zirc_proto::woke::is_woke(&message.event))
    }

    /// Whether it's a wake's line said in the chat whose message caused it, the
    /// conversation called `name` ([`zirc_proto::woke::Woke::in_its_chat`]): shown there, in
    /// among what's said, and nowhere else. Any other wake's is the agent's, which a room
    /// keeps to Everything, as it does commits and pushes, and the agent's own conversation
    /// shows.
    #[must_use]
    pub fn is_woke_in(&self, name: &str) -> bool {
        super::woke_of(self).is_some_and(|woke| woke.in_its_chat(name))
    }

    /// The id of the card it updates, when it's a live update of one ([`Message::is_live`]):
    /// how the card's steps stand now, out of history. It folds into the card further up,
    /// and is never a row, a preview or anything unread; only the latest of a card's is
    /// kept ([`super::Chat::add`]).
    #[must_use]
    pub fn live_card(&self) -> Option<&str> {
        match &self.kind {
            EntryKind::Zirc(message) if message.is_live() => match &message.event {
                zirc_proto::Event::Custom { data, .. } => data
                    .get(zirc_proto::card::KEY)
                    .and_then(|card| card.get("id"))
                    .and_then(serde_json::Value::as_str),
                _ => None,
            },
            _ => None,
        }
    }

    /// Whether it's an agent session's title card (`x-zirc/session`): what that session's
    /// card shows ([`super::Chat::agent_card`]), never something said. It's no row of a
    /// conversation, no preview and nothing unread.
    #[must_use]
    pub fn is_session_card(&self) -> bool {
        matches!(&self.kind, EntryKind::Zirc(message)
            if zirc_proto::session::SessionCard::of(&message.event).is_some())
    }

    /// Who's talking, for the kinds that are someone talking in their own words.
    #[must_use]
    pub fn speaker(&self) -> Option<&str> {
        matches!(self.kind, EntryKind::Message(_) | EntryKind::Notice(_))
            .then_some(self.from.as_str())
    }
}

/// The kinds of things that happen in a conversation. Text comes without IRC's
/// formatting codes.
#[expect(
    clippy::large_enum_variant,
    reason = "most entries are zirc messages, the large variant: boxing it would cost each of them an allocation and save no room"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    /// A message.
    Message(String),
    /// An action, as `/me` sends.
    Action(String),
    /// A notice.
    Notice(String),
    /// A zirc message. Working sets are never kept as entries.
    Zirc(Message),
    /// A zirc message that can't be read: the words it carries.
    Unreadable(String),
    /// [`Entry::from`] joined.
    Join,
    /// [`Entry::from`] left the channel, maybe saying why.
    Part(Option<String>),
    /// [`Entry::from`] left the server, maybe saying why.
    Quit(Option<String>),
    /// [`Entry::from`] removed `nick` from the channel.
    Kick {
        /// Who was removed.
        nick: String,
        /// Why, if they said.
        reason: Option<String>,
    },
    /// [`Entry::from`] is now called this.
    Nick(String),
    /// [`Entry::from`] set the topic to this.
    Topic(String),
    /// Something the client says, such as an error or a lost connection.
    Info(String),
}

/// How we attend a channel, besides being in it right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attendance {
    /// Neither of the others.
    Fresh,
    /// We were in it when the connection dropped, and haven't left it since: joined again on
    /// connecting, for a server that doesn't keep our channels (an account that isn't
    /// always-on). Leaving is the person's PART, or a KICK. A server that keeps them puts us
    /// back itself, and what it doesn't was left on another device meanwhile: [`Self::Left`].
    Rejoin,
    /// We left it (our own PART, or a KICK) and haven't joined it again: it is out of the
    /// lists, but its history stays for [`Chat::browse`].
    Left,
}

/// A conversation and what's been said in it.
#[derive(Debug, Clone)]
pub struct Conversation {
    /// The channel's name, the other nick, or the server's host.
    pub name: String,
    /// What it's a conversation with.
    pub kind: Kind,
    /// What was said, oldest first.
    pub entries: Vec<Entry>,
    /// Who is in a channel, by case-folded nick.
    pub members: BTreeMap<String, Member>,
    /// The channel's topic.
    pub topic: Option<String>,
    /// Messages from others that haven't been read: those since [`Conversation::read`]
    /// when the server keeps it, or else those that arrived live while it wasn't the one
    /// selected.
    pub unread: usize,
    /// Of those, the ones that mention us.
    pub mentions: usize,
    /// What's written in its composer and not yet sent ([`Chat::draft`]).
    pub draft: String,
    /// How many times what was written went back to its composer ([`Chat::draft_restored_in`]).
    pub draft_restored: u64,
    /// Up to when the person has read it, in any of their apps, as the server keeps it
    /// (`draft/read-marker`). `None` until the server says.
    pub read: Option<DateTime<Utc>>,
    /// Whether we're in the channel right now. Private conversations and the server's
    /// always count as joined.
    pub joined: bool,
    /// How we stand with the channel beyond being in it right now ([`Attendance`]).
    pub standing: Attendance,
    /// Counts every change to `entries`, so a view knows when to look again.
    pub revision: u64,
    /// Entries dropped from the front so far, to keep at most
    /// [`super::Options::max_entries`], so a view can tell where the rest moved.
    pub dropped: usize,
    /// Whether what it holds goes back to the start of the server's history, so there's
    /// nothing older to ask for ([`super::Chat::older`]).
    pub beginning: bool,
    /// Newer messages the server has that it doesn't, below the message with this id: what
    /// opening at an older message left between it and what's newest ([`super::Chat::hole`]).
    pub hole: Option<String>,
    /// Counts entries put in before the end or taken out, which a view can't follow
    /// piecemeal.
    pub reordered: u64,
    /// The ids of the messages it holds, so history and echoes don't show twice, and of
    /// those trimmed, so they don't come back.
    ids: HashSet<String>,
    /// What blocked people said, kept out of sight so unblocking brings it back.
    hidden: Vec<Entry>,
    /// Message id → its reactions, in the order they were first given.
    reactions: HashMap<String, Vec<Reaction>>,
    /// The reactions given, each with when, oldest first: at most [`GIVEN_KEPT`].
    given: Vec<Given>,
    /// Messages whose reactions changed since [`Conversation::take_touched`], by id.
    touched: BTreeSet<String>,
    /// Whether the last `NAMES` reply ended, so the next one starts the list afresh.
    pub(super) names_done: bool,
    /// The least a line asks of its readers for this person to see it here, as they chose
    /// in the conversation's menu ([`super::Chat::set_threshold`]): [`default_threshold`]
    /// until they do. What's below it is no row and nothing unread, but what's addressed to
    /// them always is.
    pub threshold: Level,
}

/// The least a channel shows its commits and pushes at ([`Conversation::shows`]): Show:
/// Everything, as the user chose (19 Nov 2026). What a view or a test asks for to see them.
pub const COMMITS_AND_PUSHES_SHOW_AT: Level = Level::Debug;

/// The threshold a conversation called `name` has until the person picks one: warn in
/// `#ops`, where only what needs someone belongs; info everywhere else.
#[must_use]
pub fn default_threshold(name: &str) -> Level {
    if name.eq_ignore_ascii_case(zirc_proto::state::CHANNEL) {
        Level::Warn
    } else {
        Level::Info
    }
}

impl super::Chat {
    /// What `entry`, in `conversation`, answers, when it's a reply to a message still among
    /// the entries kept: one from before the history fetched has none to quote. Who said
    /// it is named as their row's head names them ([`super::Chat::speaker`]): a person as
    /// they show, us too, never "you".
    #[must_use]
    pub fn quote(&self, conversation: &Conversation, entry: &Entry) -> Option<Quote> {
        let mut quote = conversation.quote(
            entry,
            &|owner| self.owner_name(owner),
            &|parent| self.speaker(parent).avatar,
            &|text| self.shown_text(text),
        )?;
        if let Some(parent) = conversation.entry(&quote.id) {
            quote.name = self.speaker(parent).name;
        }
        Some(quote)
    }
}

impl Conversation {
    /// Whether `entry` is shown here: at or above the [`Conversation::threshold`], or
    /// addressed to us, whatever its level. In a channel, a commit or a push
    /// ([`Entry::is_commit_or_push`]) shows only under Everything, though it stays in history
    /// at info: 771 of the agents room's 1,395 lines in 16-19 Nov 2026 were these, and the
    /// user chose to see them only there (19 Nov 2026). In an agent's own conversation, its
    /// work is whole.
    #[must_use]
    pub fn shows(&self, entry: &Entry) -> bool {
        let level = if self.kind == Kind::Channel
            && (entry.is_commit_or_push() || entry.is_woke() && !entry.is_woke_in(&self.name))
        {
            COMMITS_AND_PUSHES_SHOW_AT
        } else {
            entry.level()
        };
        entry.mention || level >= self.threshold
    }

    /// Whether `entry` counts as unread here, `me` being our nick: shown, and someone
    /// else's words ([`Entry::counts`]).
    #[must_use]
    pub fn counts(&self, entry: &Entry, me: &str) -> bool {
        self.shows(entry) && entry.counts(me)
    }

    /// Counts again what's unread, `me` being our nick, as when the threshold changed:
    /// what's after the read marker; or with none, what arrived live, never more than
    /// before. Whether it changed.
    pub(super) fn recount(&mut self, me: &str) -> bool {
        let read = self.read;
        let unread: Vec<bool> = self
            .entries
            .iter()
            .filter(|entry| {
                self.counts(entry, me)
                    && match read {
                        Some(read) => entry.time > read,
                        None => !entry.history,
                    }
            })
            .map(|entry| entry.mention)
            .collect();
        let (mut count, mut mentions) = (unread.len(), unread.iter().filter(|m| **m).count());
        // With no read marker, what arrived while the person looked was never unread, and
        // nothing here says which that was: a threshold only ever takes some away.
        if read.is_none() {
            count = count.min(self.unread);
            mentions = mentions.min(self.mentions);
        }
        let changed = (count, mentions) != (self.unread, self.mentions);
        self.unread = count;
        self.mentions = mentions;
        changed
    }

    /// Whether apps highlight `entry` as naming us: when it does, but never in a one-to-one
    /// conversation, as Slack and Discord don't, where every line counts as a mention for
    /// counts and notifications ([`Entry::mention`]) yet a highlight would mark them all.
    #[must_use]
    pub fn highlights(&self, entry: &Entry) -> bool {
        entry.mention && self.kind != Kind::Query
    }

    /// An empty conversation.
    #[must_use]
    pub fn new(name: impl Into<String>, kind: Kind) -> Self {
        let name = name.into();
        Self {
            threshold: default_threshold(&name),
            name,
            kind,
            entries: Vec::new(),
            members: BTreeMap::new(),
            topic: None,
            unread: 0,
            mentions: 0,
            draft: String::new(),
            draft_restored: 0,
            read: None,
            joined: kind != Kind::Channel,
            standing: Attendance::Fresh,
            revision: 0,
            dropped: 0,
            beginning: false,
            hole: None,
            reordered: 0,
            ids: HashSet::new(),
            hidden: Vec::new(),
            reactions: HashMap::new(),
            given: Vec::new(),
            touched: BTreeSet::new(),
            names_done: true,
        }
    }

    /// Whether it's called `name`, as IRC compares names: without case.
    #[must_use]
    pub fn is(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
    }

    /// Whether `nick` is in it.
    #[must_use]
    pub fn has_member(&self, nick: &str) -> bool {
        self.members.contains_key(&fold(nick))
    }

    /// Members, operators and the like first, then by nick.
    #[must_use]
    pub fn sorted_members(&self) -> Vec<&Member> {
        let mut members: Vec<&Member> = self.members.values().collect();
        members.sort_by_key(|member| {
            let rank = member
                .prefix
                .and_then(|p| PREFIXES.find(p))
                .unwrap_or(PREFIXES.len());
            (rank, fold(&member.nick))
        });
        members
    }

    /// Whether its list row shows `entry`: one a row shows of any conversation
    /// ([`Entry::is_shown_in_list`]) that this one shows its reader ([`Conversation::shows`]),
    /// so a row never previews a line its conversation hides, as `#ops` hides a role's
    /// report at info.
    #[must_use]
    pub fn lists(&self, entry: &Entry) -> bool {
        entry.is_shown_in_list() && self.shows(entry)
    }

    /// When the newest thing its list row shows was said ([`Conversation::lists`]): what the
    /// lists sort by, and the time its row shows.
    #[must_use]
    pub fn last_said(&self) -> Option<DateTime<Utc>> {
        self.entries
            .iter()
            .rev()
            .find(|entry| self.lists(entry))
            .map(|entry| entry.time)
    }

    /// Whether something a row would show was said in it, but none of it at or above its
    /// threshold ([`Conversation::shows`]): its row says [`super::NOTHING_AT_THIS_LEVEL`].
    #[must_use]
    pub fn all_hidden(&self) -> bool {
        self.last_said().is_none() && self.entries.iter().any(Entry::is_shown_in_list)
    }

    /// The server's id for the newest thing said in it that has one: what `/react` and
    /// `/reply` answer when no message is picked.
    #[must_use]
    pub fn newest_said_id(&self) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .filter(|entry| entry.is_said())
            .find_map(|entry| entry.id.as_deref())
    }

    /// When the newest entry the server timed happened: what reading all of it marks.
    #[must_use]
    pub fn newest_timed(&self) -> Option<DateTime<Utc>> {
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.id.is_some())
            .map(|entry| entry.time)
    }

    /// Moves [`Conversation::read`] on to `time`, if that's later, and counts again what's
    /// unread after it, `me` being our nick. Whether it moved.
    pub(super) fn mark_read(&mut self, time: DateTime<Utc>, me: &str) -> bool {
        if self.read.is_some_and(|read| read >= time) {
            return false;
        }
        self.read = Some(time);
        let unread: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|entry| entry.time > time && self.counts(entry, me))
            .collect();
        self.unread = unread.len();
        self.mentions = unread.iter().filter(|entry| entry.mention).count();
        true
    }

    /// The message with id `id`, while it's among the entries kept.
    #[must_use]
    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.id.as_deref() == Some(id))
    }

    /// What `entry` answers, as [`super::Chat::quote`] says, an agent's owner named as
    /// `owner_name` says, its words as `shown` shows them: plain, each mention by its name,
    /// as the message itself reads (`Leo Okafor heads up`, never `@leo heads up`).
    pub(crate) fn quote(
        &self,
        entry: &Entry,
        owner_name: &dyn Fn(&str) -> String,
        avatar: &dyn Fn(&Entry) -> super::Avatar,
        shown: &dyn Fn(&str) -> String,
    ) -> Option<Quote> {
        let id = entry.reply_to.as_deref()?;
        let parent = self.entry(id)?;
        let said = match &parent.kind {
            EntryKind::Message(text) | EntryKind::Action(text) | EntryKind::Notice(text) => {
                text.clone()
            }
            // An agent's post is a zirc event: its words. CI's lines, an issue's card and
            // every other event: the text they show, so a reply to one quotes something.
            EntryKind::Zirc(message) => match &message.event {
                zirc_proto::Event::Text { body } => body.clone(),
                zirc_proto::Event::Custom { text, .. } => text.clone(),
                event => event.human_text(),
            },
            _ => String::new(),
        };
        let name = super::sender_text(
            &parent.from,
            parent.owner.as_deref(),
            match &parent.kind {
                EntryKind::Zirc(message) => Some(message),
                _ => None,
            },
            owner_name,
            // [`super::Chat::quote`] names it as its row's head does.
            &str::to_owned,
        );
        let said = shown(&said);
        let first = said.lines().next().unwrap_or_default();
        let text = if first.chars().count() > QUOTE_CHARS {
            let cut: String = first.chars().take(QUOTE_CHARS - 1).collect();
            format!("{}…", cut.trim_end())
        } else {
            first.to_owned()
        };
        Some(Quote {
            id: id.to_owned(),
            from: parent.from.clone(),
            name,
            text,
            avatar: avatar(parent),
        })
    }

    /// `parts`, said as the message `id`, as one quiet line, when every app draws it so: an
    /// agent's routine work ([`super::Parts::brief`]), but never once someone has reacted to
    /// it, nor in an agent's own conversation, where its work is what there is to read (the
    /// user's choice, 14 Nov 2026). The one rule; no app decides it.
    #[must_use]
    pub fn brief(&self, parts: &super::Parts, id: Option<&str>) -> Option<String> {
        if agent(&self.name).is_some() || id.is_some_and(|id| !self.reactions(id).is_empty()) {
            return None;
        }
        parts.brief()
    }

    /// The reactions to the message with id `id`.
    #[must_use]
    pub fn reactions(&self, id: &str) -> &[Reaction] {
        self.reactions.get(id).map_or(&[], Vec::as_slice)
    }

    /// The ids of the messages whose reactions changed since the last call.
    pub fn take_touched(&mut self) -> Vec<String> {
        std::mem::take(&mut self.touched).into_iter().collect()
    }

    /// Adds `entry` in time order, keeping at most `max` entries, unless it's one already
    /// here: history and live messages overlap, and the server's ids tell them apart.
    /// Whether it was added.
    /// Whether the message the server knows as `id` is among its entries now.
    pub(super) fn has_entry(&self, id: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.id.as_deref() == Some(id))
    }

    /// Whether it holds anything after the message `id`: what's newer than a hole.
    pub(super) fn holds_after(&self, id: &str) -> bool {
        self.entries
            .iter()
            .position(|entry| entry.id.as_deref() == Some(id))
            .is_some_and(|at| at + 1 < self.entries.len())
    }

    pub(super) fn insert(&mut self, entry: Entry, max: usize) -> bool {
        if entry.is_integration_coming_or_going() {
            return false;
        }
        if let Some(id) = &entry.id
            && !self.ids.insert(id.clone())
        {
            return false;
        }
        let at = self.entries.partition_point(|e| e.time <= entry.time);
        if at < self.entries.len() {
            self.reordered += 1;
        }
        self.entries.insert(at, entry);
        // Dropped in one go once an eighth over, not one by one: each drain moves every
        // entry up, and a view starts over whenever `dropped` changes.
        if self.entries.len() > max + max / 8 {
            let extra = self.entries.len() - max;
            for gone in self.entries.drain(..extra) {
                if let Some(id) = gone.id {
                    self.reactions.remove(&id);
                }
            }
            self.dropped += extra;
        }
        self.revision += 1;
        true
    }

    /// Takes out the entries `which` picks, oldest first, for another conversation.
    pub(super) fn take(&mut self, which: impl Fn(&Entry) -> bool) -> Vec<Entry> {
        let (taken, kept) = std::mem::take(&mut self.entries)
            .into_iter()
            .partition::<Vec<_>, _>(|entry| which(entry));
        self.entries = kept;
        if !taken.is_empty() {
            self.reordered += 1;
            self.revision += 1;
        }
        taken
    }

    /// Keeps `entry`, said by someone blocked, out of sight: it isn't shown, counted or
    /// notified, and comes back if they're unblocked ([`Conversation::unhide`]).
    pub(super) fn hide(&mut self, entry: Entry) {
        if let Some(id) = &entry.id
            && !self.ids.insert(id.clone())
        {
            return;
        }
        self.hidden.push(entry);
    }

    /// Takes everything the `blocked` (folded nicks) said out of sight, and out of what's
    /// unread, `me` being our nick, `fold` how names are compared.
    pub(super) fn hide_from(&mut self, blocked: &std::collections::BTreeSet<String>, me: &str) {
        let taken = self.take(|entry| blocked.contains(&fold(&entry.from)));
        for entry in taken {
            // Unread as Chat counts it: after the read marker, or with none, arrived live.
            let unread = self.counts(&entry, me)
                && match self.read {
                    Some(read) => entry.time > read,
                    None => !entry.history,
                };
            if unread {
                self.unread = self.unread.saturating_sub(1);
                if entry.mention {
                    self.mentions = self.mentions.saturating_sub(1);
                }
            }
            self.hidden.push(entry);
        }
    }

    /// Puts back what people said who aren't in `blocked` any more, as read.
    pub(super) fn unhide(&mut self, blocked: &std::collections::BTreeSet<String>, max: usize) {
        let (back, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.hidden)
            .into_iter()
            .partition(|entry| !blocked.contains(&fold(&entry.from)));
        self.hidden = kept;
        for entry in back {
            if let Some(id) = &entry.id {
                self.ids.remove(id);
            }
            self.insert(entry, max);
        }
    }

    /// Whether it has, or had, the message with id `id`.
    #[must_use]
    pub fn holds(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    /// Notes the message `id` as changed in what's shown under it, as a reaction does, for
    /// [`Conversation::take_touched`].
    pub(super) fn touch(&mut self, id: &str) {
        self.touched.insert(id.to_owned());
    }

    /// Puts `kind`, with `formatted`, in place of what the message with id `id` says, as
    /// `from` corrected it at `at` with the message `by`: only their own message or action,
    /// said no more than [`super::EDIT_WINDOW`] before. Whether it was corrected; when it
    /// wasn't, the correction is an ordinary message.
    pub(super) fn edit(
        &mut self,
        id: &str,
        by: Option<&str>,
        from: &str,
        at: DateTime<Utc>,
        kind: EntryKind,
        formatted: Option<String>,
    ) -> bool {
        let Some(entry) = self
            .entries
            .iter_mut()
            .rev()
            .find(|entry| entry.id.as_deref() == Some(id))
        else {
            return false;
        };
        let same_kind = matches!(
            (&entry.kind, &kind),
            (EntryKind::Message(_), EntryKind::Message(_))
                | (EntryKind::Action(_), EntryKind::Action(_))
        );
        if !same_kind || fold(&entry.from) != fold(from) || at - entry.time > super::EDIT_WINDOW {
            return false;
        }
        entry.kind = kind;
        entry.formatted = formatted;
        entry.edited = true;
        // The correction itself isn't shown again should history repeat it.
        if let Some(by) = by {
            self.ids.insert(by.to_owned());
        }
        self.touched.insert(id.to_owned());
        self.revision += 1;
        true
    }

    /// Takes out the message with id `id`, trimmed from the server's history, and its
    /// reactions, `me` being our nick; it isn't put back should history repeat it. Whether
    /// it was here.
    pub(super) fn trim(&mut self, id: &str, me: &str) -> bool {
        self.ids.insert(id.to_owned());
        self.reactions.remove(id);
        self.touched.remove(id);
        let Some(at) = self
            .entries
            .iter()
            .position(|entry| entry.id.as_deref() == Some(id))
        else {
            return false;
        };
        let entry = self.entries.remove(at);
        // Unread as Chat counts it: after the read marker, or with none, arrived live.
        let unread = self.counts(&entry, me)
            && match self.read {
                Some(read) => entry.time > read,
                None => !entry.history,
            };
        if unread {
            self.unread = self.unread.saturating_sub(1);
            if entry.mention {
                self.mentions = self.mentions.saturating_sub(1);
            }
        }
        self.revision += 1;
        self.reordered += 1;
        true
    }

    /// Adds someone, `nick` possibly with a status prefix as `NAMES` gives it.
    pub(super) fn add_member(&mut self, nick: &str) {
        let (prefix, nick) = match nick.chars().next() {
            Some(c) if PREFIXES.contains(c) => (Some(c), &nick[c.len_utf8()..]),
            _ => (None, nick),
        };
        if nick.is_empty() {
            return;
        }
        self.members.insert(
            fold(nick),
            Member {
                nick: nick.to_owned(),
                prefix,
            },
        );
    }

    /// Takes someone out, saying whether they were in.
    pub(super) fn remove_member(&mut self, nick: &str) -> bool {
        self.members.remove(&fold(nick)).is_some()
    }

    /// The reactions given here, each with when, oldest first ([`Given`]).
    #[must_use]
    pub fn given(&self) -> &[Given] {
        &self.given
    }

    /// `nick` gives `emoji` to message `id` at `when`, or takes it back. Whether anything
    /// changed.
    pub(super) fn react(
        &mut self,
        id: &str,
        nick: &str,
        emoji: &str,
        on: bool,
        when: DateTime<Utc>,
    ) -> bool {
        let reactions = self.reactions.entry(id.to_owned()).or_default();
        let at = reactions.iter().position(|r| r.emoji == emoji);
        let changed = match (at, on) {
            (Some(at), true) if !reactions[at].by(nick) => {
                reactions[at].nicks.push(nick.to_owned());
                true
            }
            (None, true) => {
                reactions.push(Reaction {
                    emoji: emoji.to_owned(),
                    nicks: vec![nick.to_owned()],
                });
                true
            }
            (Some(at), false) if reactions[at].by(nick) => {
                reactions[at]
                    .nicks
                    .retain(|n| !n.eq_ignore_ascii_case(nick));
                if reactions[at].nicks.is_empty() {
                    reactions.remove(at);
                }
                true
            }
            _ => false,
        };
        if changed {
            self.touched.insert(id.to_owned());
            if on {
                if self.given.len() >= GIVEN_KEPT {
                    self.given.remove(0);
                }
                self.given.push(Given {
                    id: id.to_owned(),
                    nick: nick.to_owned(),
                    emoji: emoji.to_owned(),
                    at: when,
                });
            } else {
                self.given.retain(|given| {
                    !(given.id == id
                        && given.emoji == emoji
                        && given.nick.eq_ignore_ascii_case(nick))
                });
            }
        }
        changed
    }
}

/// `name` as IRC compares it: ASCII without case.
#[must_use]
pub fn fold(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// Whether `target` names a channel rather than a nick.
#[must_use]
pub fn is_channel(target: &str) -> bool {
    target.starts_with(['#', '&'])
}

/// What separates a machine's nick from one of its agent sessions in the name of a
/// conversation with that session, `mattias-mbp/cc-1531f8`. No nick has one.
pub const SESSION_MARK: char = '/';

/// The machine and the agent session working there that a private conversation called
/// `name` is with, when it's one: `("mattias-mbp", "cc-1531f8")` for `mattias-mbp/cc-1531f8`.
#[must_use]
pub fn agent(name: &str) -> Option<(&str, &str)> {
    if is_channel(name) {
        return None;
    }
    let (nick, session) = name.split_once(SESSION_MARK)?;
    (!nick.is_empty() && !session.is_empty()).then_some((nick, session))
}

/// Who a conversation called `name` talks to on the server: a channel, a nick, or for a
/// conversation with an agent session, its machine's nick.
#[must_use]
pub fn target(name: &str) -> &str {
    agent(name).map_or(name, |(nick, _)| nick)
}

/// The words that send `text` to `session` on its machine: zircd hands any line its owner
/// says to the machine to its agents, and to `session` alone when the first word names it.
/// No `/agent` leads it any more; zircd still takes one from older apps.
#[must_use]
pub fn to_agent(session: &str, text: &str) -> String {
    format!("{session} {text}")
}

/// The conversation with the agent `session` on the machine `nick`: `<machine>/<session>`,
/// as [`agent`] reads it.
#[must_use]
pub fn agent_channel(nick: &str, session: &str) -> String {
    format!("{nick}{SESSION_MARK}{session}")
}

/// Whom `text`, said to a machine, names first, and the rest: `<session> …`, with the
/// `/agent` older apps put before it left out. None for nothing but `/agent`.
#[must_use]
pub fn named_session(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    let text = match text.strip_prefix("/agent") {
        Some(rest) if rest.starts_with(char::is_whitespace) => rest.trim_start(),
        _ => text,
    };
    let (first, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    (!first.is_empty()).then(|| (first, rest.trim_start()))
}

/// What `text`, said to a machine, says to `session` on it, when it's for that session:
/// `text` without its `<session>` (and the `/agent` older apps put before it).
#[must_use]
pub fn for_agent<'a>(text: &'a str, session: &str) -> Option<&'a str> {
    named_session(text)
        .filter(|(first, _)| *first == session)
        .map(|(_, rest)| rest)
}

/// Whether `nick` is an integration's ([`zirc_proto::names::is_integration_nick`]), whatever
/// case the server gives it in.
fn is_integration(nick: &str) -> bool {
    zirc_proto::names::is_integration_nick(&nick.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    fn at(minute: u32, id: Option<&str>, text: &str) -> Entry {
        Entry {
            id: id.map(str::to_owned),
            ..Entry::info(
                Utc.with_ymd_and_hms(2026, 10, 30, 12, minute, 0).unwrap(),
                text.into(),
            )
        }
    }

    fn texts(conversation: &Conversation) -> Vec<&str> {
        conversation
            .entries
            .iter()
            .map(|e| match &e.kind {
                EntryKind::Info(text) => text.as_str(),
                _ => "",
            })
            .collect()
    }

    #[test]
    fn shows_commits_and_pushes_only_under_everything() {
        let entry = |event: zirc_proto::Event| Entry {
            kind: EntryKind::Zirc(zirc_proto::Message::new("cc-1", event)),
            ..at(0, None, "")
        };
        let push: zirc_proto::Event = serde_json::from_str(
            r#"{"type": "push", "branch": "login", "head": "abc1234", "forced": false}"#,
        )
        .unwrap();
        let push = entry(push);
        let taking = entry(zirc_proto::Event::Text {
            body: "taking: login retries".into(),
        });
        let mut agents = Conversation::new("#web/agents", Kind::Channel);
        assert!(!agents.shows(&push) && agents.shows(&taking));
        agents.threshold = COMMITS_AND_PUSHES_SHOW_AT;
        assert!(agents.shows(&push));
        // Its own conversation shows an agent's work whole.
        assert!(Conversation::new("ada-mbp/cc-1", Kind::Query).shows(&push));
    }

    #[test]
    fn sends_to_an_agent_without_a_leading_agent() {
        assert_eq!(to_agent("cc-1", "run the tests"), "cc-1 run the tests");
    }

    #[test]
    fn reads_what_was_said_to_an_agent_with_or_without_the_older_prefix() {
        assert_eq!(for_agent("cc-1 run it", "cc-1"), Some("run it"));
        assert_eq!(for_agent("/agent cc-1 run it", "cc-1"), Some("run it"));
        assert_eq!(for_agent("/agent  cc-1", "cc-1"), Some(""));
        assert_eq!(for_agent("cc-12 run it", "cc-1"), None);
        assert_eq!(for_agent("/agent cc-2 run it", "cc-1"), None);
        assert_eq!(for_agent("/agentcc-1 run it", "cc-1"), None);
        assert_eq!(for_agent("hello", "cc-1"), None);
        assert_eq!(
            named_session("/agent cc-1 run it"),
            Some(("cc-1", "run it"))
        );
        assert_eq!(named_session("  cc-1"), Some(("cc-1", "")));
        assert_eq!(named_session("/agent "), None);
        assert_eq!(
            agent(&agent_channel("me-mbp", "cc-1")),
            Some(("me-mbp", "cc-1"))
        );
    }

    #[test]
    fn keeps_entries_in_time_order_without_repeats() {
        let mut api = Conversation::new("#api", Kind::Channel);
        assert!(api.insert(at(5, Some("b"), "second"), 10));
        assert!(api.insert(at(1, Some("a"), "first"), 10));
        assert!(!api.insert(at(5, Some("b"), "second again"), 10));
        assert!(api.insert(at(9, None, "third"), 10));
        assert_eq!(texts(&api), ["first", "second", "third"]);
        assert_eq!((api.revision, api.reordered), (3, 1));
    }

    #[test]
    fn keeps_at_most_so_many_entries() {
        let mut api = Conversation::new("#api", Kind::Channel);
        for minute in 0..5 {
            api.insert(at(minute, None, &minute.to_string()), 3);
        }
        assert_eq!(texts(&api), ["2", "3", "4"]);
        assert_eq!(api.dropped, 2);
    }

    #[test]
    fn sorts_members_by_status_then_nick() {
        let mut api = Conversation::new("#api", Kind::Channel);
        for nick in ["bob", "+carol", "@Alice", "dave", "~eve"] {
            api.add_member(nick);
        }
        let nicks: Vec<_> = api
            .sorted_members()
            .iter()
            .map(|m| m.nick.as_str())
            .collect();
        assert_eq!(nicks, ["eve", "Alice", "carol", "bob", "dave"]);
        assert!(api.has_member("alice") && api.remove_member("ALICE"));
    }

    #[test]
    fn counts_reactions_once_per_nick() {
        let mut api = Conversation::new("#api", Kind::Channel);
        assert!(api.react("m1", "alice", "👍", true, Utc::now()));
        assert!(!api.react("m1", "Alice", "👍", true, Utc::now()));
        assert!(api.react("m1", "bob", "👍", true, Utc::now()));
        assert_eq!(api.reactions("m1")[0].nicks, ["alice", "bob"]);
        assert!(api.react("m1", "alice", "👍", false, Utc::now()));
        assert!(api.react("m1", "bob", "👍", false, Utc::now()));
        assert!(api.reactions("m1").is_empty());
        assert_eq!(api.take_touched(), ["m1"]);
        assert!(api.take_touched().is_empty());
    }
}
