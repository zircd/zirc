//! A conversation's lines as every app's records carry them: what [`Chat::items`] lays out,
//! each with its key, times on the view's clock, its day divider, who said it and their face,
//! its rich text cut when long, its reactions and quote, and a system line's or a file's
//! parts, all worded here. The phones take them over UniFFI and the web client as JSON, and
//! only draw them.

use std::collections::HashMap;

use chrono::{DateTime, FixedOffset, Utc};
use serde::Serialize;
use ts_rs::TS;

use super::conversation::{Conversation, Entry, agent, fold};
use super::face::{Face, LineSpeaker};
use super::flat::{RichBlock, RichSpan, is_plain};
use super::rich::Block;
use super::view::{Item, Landing, Parts};
use super::{Chat, Icon, Presence, Standing, activity_conversation, file, first_link, link_blocks};

/// The largest an image or a video's poster is drawn, in points: wider ones are fitted.
pub const MEDIA_WIDTH: u32 = 360;

/// The tallest an image or a video's poster is drawn, in points.
pub const MEDIA_HEIGHT: u32 = 240;

/// How many times an animated picture, such as a GIF, plays before it rests on its last
/// frame, as Slack's do; a tap on one that rests plays it again (the user's choice, 13 Nov
/// 2026, for every app).
pub const ANIMATION_PLAYS: u32 = 3;

/// What sort of line it is, for how it's drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum LineKind {
    /// Something someone said.
    Chat,
    /// A `/me` action: `alice waves`.
    Action,
    /// A notice: a message that expects no answer.
    Notice,
    /// A zirc event, or several people changing the same files: [`Line::system`] has its
    /// parts.
    Event,
    /// How a landing went, or any card: one line for each, which later events update.
    /// [`Line::system`] has its outcome and buttons, [`Line::rich`] its words.
    Landing,
    /// People who came and went, as one quiet line.
    Membership,
    /// A quiet line: a topic, a new nick, someone removed, what the app says, or a file
    /// shared from elsewhere, which isn't loaded.
    Note,
    /// A file someone shared: [`Line::file`] has what to draw.
    File,
}

/// One line of a conversation, as every app draws it.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is a fact of its own about the line"
)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct Line {
    /// What stays the same for it however the list changes: the server's id, or else its
    /// time and how many id-less lines came before it at that time, `1730289600000:0`.
    pub key: String,
    /// The server's id, which reactions and replies name.
    pub id: Option<String>,
    /// In an agent session's conversation ([`Chat::agent_lines`]), the room a line of its
    /// activity was said in, `#web/agents`, for the muted chip after its time.
    pub room: Option<String>,
    /// When, in milliseconds since 1970.
    pub time_ms: i64,
    /// That on the view's clock: `15:48` ([`super::clock`]).
    pub clock: String,
    /// The day divider above it, `Today` or `Thursday 29 October 2026`, when it's the first
    /// line of its day ([`super::day_label`]).
    pub day: Option<String>,
    /// What sort of line it is.
    pub kind: LineKind,
    /// Whether it carries on a run from the same sender, so its avatar and name go.
    pub continues: bool,
    /// Who: their nick, which replies, mentions and their card name. Empty for what no one
    /// said, such as people coming and going.
    pub from: String,
    /// Who they are, as its head shows them; `None` for a quiet line.
    pub speaker: Option<LineSpeaker>,
    /// Their dot: here in the conversation, away or gone; `None` for a quiet line.
    pub presence: Option<Presence>,
    /// Whether they're a guest, let into a public channel without an account.
    pub guest: bool,
    /// What it says, whole, for copying and the menu.
    pub text: String,
    /// What it says as blocks to draw: code, quotes, lists, bold and the like. Empty when
    /// it's one plain paragraph shown whole, which [`Line::text`] says, or for a post (an
    /// event with [`Icon::Post`]), [`SystemLine::details`], whose words these are when not.
    pub rich: Vec<RichBlock>,
    /// The first web address it has, for Copy link: a landing's page, else its build's.
    pub link: Option<String>,
    /// Whether it names us, so it stands out: never in a one-to-one conversation, where
    /// every line is for us.
    pub mention: bool,
    /// Whether it's our own.
    pub mine: bool,
    /// The zirc app it was sent from, `desktop`, where that's worth showing
    /// ([`super::shows_sent_from`]).
    pub sent_from: Option<String>,
    /// Whether its author corrected it, which apps say after its words as
    /// [`super::EDITED`] does.
    pub edited: bool,
    /// Its reactions, in the order given.
    pub reactions: Vec<LineReaction>,
    /// For a reply, what it answers, to show above it.
    pub quote: Option<LineQuote>,
    /// For a long message, its one "Show N more lines" control: [`Line::rich`] is then its
    /// start, or all of it while it's open ([`Chat::body`]).
    pub more: Option<LineMore>,
    /// For an event, a landing or people coming and going, its parts.
    pub system: Option<SystemLine>,
    /// For a file, what to draw.
    pub file: Option<LineFile>,
    /// What can be done with it, which its hover bar, menu and long-press sheet offer.
    pub actions: LineActions,
}

/// What can be done with a line, which every app's hover bar, right-click menu and
/// long-press sheet offer, and only these. An agent's event or a landing's card offers what
/// a message does, but deleting: a reaction on one is kept and shown as on a message, by the
/// server's id it has as any `PRIVMSG` does. Copying its text needs no field: every line
/// offers that.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is an action of its own"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct LineActions {
    /// Reacting to it: the hover bar's emoji, and the picker for the rest.
    pub react: bool,
    /// Replying to it, which quotes it.
    pub reply: bool,
    /// Copying its own link, which opens it in every app, where its conversation has links.
    pub copy_link: bool,
    /// Correcting it ([`Chat::say_edit`]): only our own message or action, for
    /// [`super::EDIT_WINDOW`] after it was said.
    pub edit: bool,
    /// Deleting it for everyone (an IRCv3 `REDACT`): only our own.
    pub delete: bool,
}

impl LineActions {
    /// What a line of `kind` offers: with the server's `id`, reacting, replying and its link,
    /// unless it's a quiet line (people coming and going, a note); deleting too when it's
    /// `mine` and something said: a message, an action, a notice or a file, never an event,
    /// which its machine's zircd says, nor a card; correcting it when `editable`
    /// ([`super::editable`]).
    #[must_use]
    pub fn of(kind: LineKind, id: Option<&str>, mine: bool, editable: bool) -> Self {
        let named = id.is_some() && !matches!(kind, LineKind::Membership | LineKind::Note);
        let said = matches!(
            kind,
            LineKind::Chat | LineKind::Action | LineKind::Notice | LineKind::File
        );
        Self {
            react: named,
            reply: named,
            copy_link: named,
            edit: named && editable,
            delete: named && said && mine,
        }
    }
}

/// One emoji given to a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct LineReaction {
    /// The emoji.
    pub emoji: String,
    /// How many gave it.
    pub count: u32,
    /// Whether we did.
    pub mine: bool,
}

/// What a reply answers, shown above it as `↪ bob: first line…` and jumped to by its id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct LineQuote {
    /// The answered message's id, as its [`Line::key`].
    pub id: String,
    /// Who said it, as their messages' heads name them. Empty when the message is no longer
    /// among those kept, which apps show as "a reply to an earlier message".
    pub from: String,
    /// Its first line, cut short; empty as `from` is.
    pub text: String,
    /// Who said it, as their messages' heads show them; `None` as `from` is empty.
    pub face: Option<Face>,
}

/// A long message's one control ([`super::More`]): a text button on its own line under the
/// cut, or in a cut code block's footer beside Copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct LineMore {
    /// What it says: `Show 34 more lines`, or `Show less` while open.
    pub label: String,
    /// What a screen reader says: `Show 34 more lines of Maya Chen's agent's message`.
    pub spoken: String,
    /// How many lines the cut hides, code included.
    pub hidden: u32,
    /// Whether the message shows all of itself; [`Chat::toggle_more`] takes it.
    pub open: bool,
    /// While cut inside a code block, the last of [`Line::rich`]: how many of its first
    /// lines show, the last fading into the code's ground. `None` when the cut is in its
    /// words, or it's open.
    pub code_lines: Option<u32>,
}

/// An agent session, by what it goes by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SessionName {
    /// Its id, `cc-598aa1`, shown on hover or a long press.
    pub id: String,
    /// Its name, `zirc-chat`, as [`Chat::session_label`] says, or else its id.
    pub label: String,
}

/// A system line in parts ([`Parts`]): headed as a message is, by who, whose agent and the
/// time, never its kind; then its lead (marker and verb), its details and the paths under
/// them.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is a fact of its own about the line, which every app reads as is"
)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SystemLine {
    /// Its icon.
    pub icon: Icon,
    /// What to draw for it, one character wide ([`Icon::glyph`]).
    pub glyph: String,
    /// What its body starts with ([`Parts::lead`]): its marker and what was done, `✎ is
    /// about to change`; the marker alone when nothing was, and nothing for a post. Draw
    /// it, never `glyph` and `verb` put together.
    pub lead: String,
    /// Who, as people read it: an agent session's name, a machine itself as `dana's agent`,
    /// a person's nick. Never a device.
    pub actor: String,
    /// The agent session it's from, or a landing's, shown dim by its name, its id on hover.
    pub session: Option<SessionName>,
    /// For a post to an agent session, that session: shown `→ zirc-chat` before the details.
    pub to: Option<SessionName>,
    /// What they did, in a few words: `is about to change`; empty for a post.
    pub verb: String,
    /// What it was: an intent's summary, a commit's subject, a post's words.
    pub details: String,
    /// The paths it's about, set in monospace.
    pub paths: Vec<String>,
    /// Whether its lead, paths and details read as one phrase, wrapped together: a file two
    /// sessions are changing, `! also changing a.rs with cc-1`. Apps draw them so, and
    /// never decide it from the icon.
    pub one_phrase: bool,
    /// Whether its paths are listed under its words, one a line, the first
    /// [`SHOWN_PATHS`](super::SHOWN_PATHS) and a "Show N more"
    /// ([`show_more_paths_label`](super::show_more_paths_label)) for the rest.
    pub paths_listed: bool,
    /// Whether its details start a line of their own under its lead rather than following
    /// it on the same line: when its paths are listed under them, or they have lines of
    /// their own, as a push's commits do.
    pub details_apart: bool,
    /// [`SystemLine::lead`] with its links ([`Chat::linked`]). Empty when none of it links.
    pub lead_links: Vec<RichSpan>,
    /// [`SystemLine::verb`] with its links: a push's branch and commit to their pages on the
    /// forge. Empty when none of it links.
    pub verb_links: Vec<RichSpan>,
    /// [`SystemLine::details`] with its links. Empty when none of it links.
    pub details_links: Vec<RichSpan>,
    /// Whether it's drawn as one quiet line: only people coming and going are.
    pub compact: bool,
    /// The event as one muted line, for an app that draws an agent's routine work quietly
    /// ([`Parts::brief`](super::Parts::brief)): `checkout is about to change Coupon.tsx`.
    /// `None` for what keeps its full form.
    pub brief: Option<String>,
    /// When the last of a fold happened, on the view's clock, when it's later than the
    /// first: a commit and its push, made one line.
    pub until_clock: Option<String>,
    /// How a landing went, as an icon before its words; `None` for anything else.
    pub outcome: Option<Icon>,
    /// Whether it's a landing's card: its branch, build and stages on one line, which
    /// [`Line::rich`] draws, updated in place by later events.
    pub card: bool,
    /// A card's buttons, those it shows us ([`Chat::shows_action`]), when it has any.
    pub buttons: Option<CardButtons>,
    /// What tells this version of a card apart from the next ([`Landing::card_version`]),
    /// so what a press came to shows until the integration posts the card again; `None`
    /// with no card.
    pub card_version: Option<String>,
}

/// A card's buttons ([`zirc_proto::card`]): which card, by its sender and id, and the
/// actions it shows us, which press through zirc.dev.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CardButtons {
    /// The nick that posted the card, whose integration a press reaches.
    pub sender: String,
    /// The card's id.
    pub card: String,
    /// Tells its versions apart, so what a press came to shows until the card changes: the
    /// same as [`SystemLine::card_version`].
    pub key: String,
    /// Its buttons, in order.
    pub actions: Vec<CardAction>,
}

/// One of a card's buttons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CardAction {
    /// The integration's own name for it.
    pub id: String,
    /// What the button says.
    pub label: String,
    /// What to ask before pressing it, if anything.
    pub confirm: Option<String>,
}

/// How a shared file is drawn ([`file::shape`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum FileShape {
    /// Inline, resized by zirc.dev.
    Image,
    /// A poster and a player.
    Video,
    /// A card with the start of its text, which zirc.dev previews.
    Text,
    /// A card with its name, size and a download button.
    Other,
}

impl From<file::Shape> for FileShape {
    fn from(shape: file::Shape) -> Self {
        match shape {
            file::Shape::Image => Self::Image,
            file::Shape::Video => Self::Video,
            file::Shape::Text => Self::Text,
            file::Shape::Other => Self::Other,
        }
    }
}

/// A file someone shared, hosted by the web app, as every app draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct LineFile {
    /// Its name.
    pub name: String,
    /// Its media type, `image/jpeg`.
    pub mime: String,
    /// Its size in bytes.
    pub size: u64,
    /// That as people read it: `2.3 MB` ([`file::size_text`]).
    pub size_text: String,
    /// How it's drawn.
    pub shape: FileShape,
    /// Where it's downloaded or played: zirc.dev sends a signed-in browser on to it.
    pub url: Option<String>,
    /// The width to draw an image or a video's poster at, in points, fitted to
    /// [`MEDIA_WIDTH`] by [`MEDIA_HEIGHT`], when its size is known.
    pub width: Option<u32>,
    /// The height to draw it at, as `width`.
    pub height: Option<u32>,
    /// An image's, or a video's poster's, sizes zirc.dev resizes it to, as an `<img>`'s
    /// `srcset`: `<url>?w=64 64w, …`.
    pub srcset: Option<String>,
    /// A video's poster, sharp on a screen of two pixels a point.
    pub poster: Option<String>,
}

impl Chat {
    /// The conversation `name`'s lines, oldest first, as every app shows it, times on a
    /// clock `offset` from UTC; with `people_only`, only what people said and what mentions
    /// us, as a conversation's "People only" filter shows it ([`super::people_only`]).
    /// Empty for a conversation there isn't.
    #[must_use]
    pub fn lines(&self, name: &str, offset: FixedOffset, people_only: bool) -> Vec<Line> {
        let Some(conversation) = self.conversation(name) else {
            return Vec::new();
        };
        Layout::new(self, conversation, offset, HashMap::new()).lines(people_only)
    }

    /// The agent session `name`'s conversation (`<machine>/<session>`) as its lines: its
    /// activity in every room we're in and what we and it said to each other
    /// ([`Chat::agent_activity`]), each line of activity with its room. As its own: its
    /// lines leave out whose agent it is, as the header says it, and a line from another
    /// room than the one before starts a new run, with its own head and chip, so each chip
    /// is true for its run. Empty for any other conversation.
    #[must_use]
    pub fn agent_lines(&self, name: &str, offset: FixedOffset) -> Vec<Line> {
        let Some((_, session)) = agent(name) else {
            return Vec::new();
        };
        let activity = self.agent_activity(name);
        let conversation = activity_conversation(name, &activity);
        let rooms = conversation
            .entries
            .iter()
            .zip(activity)
            .filter_map(|(entry, line)| Some((std::ptr::from_ref(entry), line.room?)))
            .collect();
        let mut lines = Layout::new(self, &conversation, offset, rooms).lines(false);
        let mut last: Option<String> = None;
        for line in &mut lines {
            if line.day.is_some() {
                last = None;
            }
            let Some(speaker) = &mut line.speaker else {
                continue;
            };
            if speaker.session.as_deref() == Some(session) {
                speaker.detail = None;
            }
            if line.continues && line.room != last {
                line.continues = false;
            }
            last.clone_from(&line.room);
        }
        lines
    }
}

/// Puts a conversation's items into lines.
struct Layout<'a> {
    chat: &'a Chat,
    conversation: &'a Conversation,
    offset: FixedOffset,
    /// The room each line of an agent's activity was said in, by its entry.
    rooms: HashMap<*const Entry, String>,
    keys: Keys,
}

impl<'a> Layout<'a> {
    fn new(
        chat: &'a Chat,
        conversation: &'a Conversation,
        offset: FixedOffset,
        rooms: HashMap<*const Entry, String>,
    ) -> Self {
        Self {
            chat,
            conversation,
            offset,
            rooms,
            keys: Keys::default(),
        }
    }

    fn lines(mut self, people_only: bool) -> Vec<Line> {
        let offset = self.offset;
        let items = self.chat.items(self.conversation, |time| {
            time.with_timezone(&offset).date_naive()
        });
        let items = if people_only {
            super::people_only(items, self.chat.me())
        } else {
            items
        };
        let now = (self.chat.clock)();
        let today = now.with_timezone(&offset).date_naive();
        let mut lines = Vec::with_capacity(items.len());
        let mut day = None;
        for item in items {
            if let Item::Day(date) = item {
                day = Some(super::day_label(date, today));
                continue;
            }
            let editable = item
                .said_entry()
                .is_some_and(|entry| super::editable(entry, self.chat.me(), now));
            let mut line = self.line(item);
            line.day = day.take();
            line.actions = LineActions::of(line.kind, line.id.as_deref(), line.mine, editable);
            lines.push(line);
        }
        lines
    }

    /// `item`'s line; never a day, which is the divider on the line after it.
    fn line(&mut self, item: Item<'_>) -> Line {
        match item {
            Item::Day(_) => unreachable!("a day goes on the line after it"),
            Item::Message {
                entry,
                text,
                continues,
                sent_from,
                show_sent_from,
            } => {
                let mut line = self.said(entry, LineKind::Chat, text, continues);
                line.sent_from = sent_from.filter(|_| show_sent_from).map(str::to_owned);
                self.body(&mut line, entry.rich());
                line
            }
            Item::Notice {
                entry,
                text,
                continues,
            } => {
                let mut line = self.said(entry, LineKind::Notice, text, continues);
                self.body(&mut line, entry.rich());
                line
            }
            Item::Action {
                entry,
                text,
                continues,
            } => self.said(entry, LineKind::Action, text, continues),
            Item::Event {
                entry,
                text,
                parts,
                continues,
            } => {
                let mut line = self.said(entry, LineKind::Event, &text, continues);
                // A post's words read as a message's: drawn, and cut when long, as one is.
                if parts.icon == Icon::Post && !parts.details.is_empty() {
                    self.body(&mut line, super::rich(&parts.details));
                }
                line.system = Some(self.system(&parts, parts.session.clone()));
                line
            }
            Item::Landing(landing) => self.landing(&landing),
            Item::File {
                entry,
                file,
                from,
                continues,
            } => self.file(entry, file, &from, continues),
            Item::Note { entry, text } => {
                let key = self.keys.of(entry);
                let mut line = self.blank(key, Some(entry), entry.time, LineKind::Note);
                line.text = text;
                line
            }
            Item::Membership(membership) => {
                let key = self.keys.next(membership.time.timestamp_millis());
                let mut line = self.blank(key, None, membership.time, LineKind::Membership);
                line.text = membership.text();
                line.system = Some(self.system(&membership.parts(), None));
                line
            }
        }
    }

    /// A line with only its key, id, room, time and kind.
    fn blank(
        &self,
        key: String,
        entry: Option<&Entry>,
        time: DateTime<Utc>,
        kind: LineKind,
    ) -> Line {
        Line {
            key,
            id: entry.and_then(|entry| entry.id.clone()),
            room: entry.and_then(|entry| self.rooms.get(&std::ptr::from_ref(entry)).cloned()),
            time_ms: time.timestamp_millis(),
            clock: self.clock(time),
            day: None,
            kind,
            continues: false,
            from: entry.map(|entry| entry.from.clone()).unwrap_or_default(),
            speaker: None,
            presence: None,
            guest: false,
            text: String::new(),
            rich: Vec::new(),
            link: None,
            mention: false,
            mine: false,
            sent_from: None,
            edited: false,
            reactions: Vec::new(),
            quote: None,
            more: None,
            system: None,
            file: None,
            actions: LineActions::default(),
        }
    }

    /// What someone said or did, `entry`, headed by them: who, their face and dot, whether
    /// it names us or is ours, its reactions and what it answers.
    fn said(&mut self, entry: &Entry, kind: LineKind, text: &str, continues: bool) -> Line {
        let key = self.keys.of(entry);
        let nick = entry.from.as_str();
        let me = self.chat.me();
        Line {
            continues,
            speaker: (!nick.is_empty()).then(|| self.chat.line_speaker(entry)),
            presence: (!nick.is_empty()).then(|| self.presence(nick)),
            guest: self.chat.is_guest(nick),
            text: text.to_owned(),
            link: first_link(text).map(str::to_owned),
            mention: self.conversation.highlights(entry),
            mine: !me.is_empty() && fold(nick) == fold(me),
            edited: entry.edited,
            reactions: self.reactions(entry),
            quote: self.quote(entry),
            ..self.blank(key, Some(entry), entry.time, kind)
        }
    }

    /// `line`'s words as `blocks`, drawn and cut when long, with its control: none when
    /// they're one plain paragraph shown whole, which its text says.
    fn body(&self, line: &mut Line, blocks: Vec<Block>) {
        let shown = self.chat.body(&line.key, blocks, line.mention);
        let more = shown.more.map(|more| {
            // Whose message, as its head says: `Maya Chen's agent`, or the person's name.
            let whose = line
                .speaker
                .as_ref()
                .map(|speaker| speaker.detail.clone().unwrap_or(speaker.name.clone()))
                .unwrap_or_default();
            LineMore {
                label: more.label(),
                spoken: more.spoken(&whose),
                hidden: count(more.hidden),
                open: more.open,
                code_lines: more.code_lines.map(count),
            }
        });
        let plain = more.is_none() && is_plain(&shown.blocks);
        let rich = self.chat.flat_in(&self.conversation.name, shown.blocks);
        // Plain words draw as text, unless they name code that links.
        if plain && !names_code(&rich) {
            return;
        }
        line.rich = rich;
        line.more = more;
    }

    /// A system line from `parts`, about the agent session `session`, if any.
    fn system(&self, parts: &Parts, session: Option<String>) -> SystemLine {
        let lead = parts.lead();
        let one_phrase = parts.icon == Icon::Overlap && !parts.paths.is_empty();
        let paths_listed = !one_phrase && !parts.paths.is_empty();
        let details_apart = !one_phrase
            && !lead.is_empty()
            && !parts.details.is_empty()
            && (paths_listed || parts.details.contains('\n'));
        SystemLine {
            one_phrase,
            paths_listed,
            details_apart,
            icon: parts.icon,
            glyph: parts.icon.glyph().to_owned(),
            lead_links: self.chat.linked(&lead, &parts.links),
            lead,
            actor: parts.actor.clone(),
            session: session.map(|id| self.session(id)),
            to: parts.to.clone().map(|id| self.session(id)),
            verb_links: self.chat.linked(&parts.verb, &parts.links),
            details_links: self.chat.linked(&parts.details, &parts.links),
            verb: parts.verb.clone(),
            details: parts.details.clone(),
            paths: parts.paths.clone(),
            compact: parts.compact,
            brief: parts.brief(),
            until_clock: (parts.until != parts.time).then(|| self.clock(parts.until)),
            outcome: None,
            card: false,
            buttons: None,
            card_version: None,
        }
    }

    /// How a landing went, or any card, as CI or an integration posts it: one line, its
    /// words the card's line or what was said, with its branch and commit linked, and its
    /// outcome and buttons.
    fn landing(&mut self, landing: &Landing<'_>) -> Line {
        let entry = landing.entry;
        let parts = landing.parts();
        let card_line = landing.card_line();
        // For copying: the card's line as it reads, or what CI said.
        let text = card_line.as_ref().map_or_else(
            || landing.said().to_owned(),
            |line| line.iter().map(|span| span.text.as_str()).collect(),
        );
        let mut line = self.said(entry, LineKind::Landing, &text, landing.continues);
        line.rich = self.chat.flat(
            self.chat
                .drawn(link_blocks(landing.rich(), &landing.links())),
        );
        line.link = landing
            .page
            .clone()
            .or_else(|| landing.url.map(str::to_owned));
        let outcome = (parts.icon != Icon::Post).then_some(parts.icon);
        let version = landing.card_version();
        line.system = Some(SystemLine {
            outcome,
            card: card_line.is_some(),
            buttons: self.buttons(landing, &version),
            card_version: landing.card.as_ref().map(|_| version),
            ..self.system(&parts, landing.session.map(str::to_owned))
        });
        line
    }

    /// A card's buttons, those it shows us, when it has any.
    fn buttons(&self, landing: &Landing<'_>, version: &str) -> Option<CardButtons> {
        let card = landing.card.as_ref()?;
        let actions: Vec<CardAction> = card
            .actions
            .iter()
            .filter(|action| self.chat.shows_action(card, action))
            .map(|action| CardAction {
                id: action.id.clone(),
                label: action.label.clone(),
                confirm: action.confirm.clone(),
            })
            .collect();
        (!actions.is_empty()).then(|| CardButtons {
            sender: landing.entry.from.clone(),
            card: card.id.clone(),
            key: version.to_owned(),
            actions,
        })
    }

    /// A shared file: drawn when the web app hosts it, otherwise only said, as a note,
    /// loading nothing. `from` is who shared it, as plain text names them.
    fn file(
        &mut self,
        entry: &Entry,
        file: &zirc_proto::File,
        from: &str,
        continues: bool,
    ) -> Line {
        let size_text = file::size_text(file.size);
        let hosted = self
            .chat
            .files_web()
            .is_some_and(|web| file::is_hosted(file, web));
        if !hosted {
            let key = self.keys.of(entry);
            let mut line = self.blank(key, Some(entry), entry.time, LineKind::Note);
            line.text = format!(
                "{from} shared {} ({size_text}) from elsewhere, not shown",
                file.name
            );
            return line;
        }
        let shape = FileShape::from(file::shape(file));
        let drawn = matches!(shape, FileShape::Image | FileShape::Video);
        let fitted =
            file::fit(file.width, file.height, MEDIA_WIDTH, MEDIA_HEIGHT).filter(|_| drawn);
        let mut line = self.said(entry, LineKind::File, &file.name, continues);
        line.link = None;
        line.file = Some(LineFile {
            name: file.name.clone(),
            mime: file.mime.clone(),
            size: file.size,
            size_text,
            shape,
            url: Some(file.url.clone()),
            width: fitted.map(|(width, _)| width),
            height: fitted.map(|(_, height)| height),
            srcset: drawn.then(|| srcset(file)),
            poster: (shape == FileShape::Video).then(|| file::image_url(file, MEDIA_WIDTH * 2)),
        });
        line
    }

    /// Whether `nick` is around, as the dot on their face says: here in the conversation,
    /// away or gone ([`Standing::of`]).
    fn presence(&self, nick: &str) -> Presence {
        let here = self.conversation.members.contains_key(&fold(nick));
        Standing::of(here, self.chat.away(nick)).presence()
    }

    fn reactions(&self, entry: &Entry) -> Vec<LineReaction> {
        let Some(id) = entry.id.as_deref() else {
            return Vec::new();
        };
        let me = self.chat.me();
        self.conversation
            .reactions(id)
            .iter()
            .map(|reaction| LineReaction {
                emoji: reaction.emoji.clone(),
                count: count(reaction.nicks.len()),
                mine: reaction.by(me),
            })
            .collect()
    }

    /// What `entry` answers, when it's a reply: who said it and its first line, named and
    /// drawn as their messages are; only its id when it's no longer among those kept.
    fn quote(&self, entry: &Entry) -> Option<LineQuote> {
        let id = entry.reply_to.as_ref()?;
        Some(self.chat.quote(self.conversation, entry).map_or_else(
            || LineQuote {
                id: id.clone(),
                from: String::new(),
                text: String::new(),
                face: None,
            },
            |quote| LineQuote {
                face: Some(self.chat.face(&quote.avatar)),
                id: quote.id,
                from: quote.name,
                text: quote.text,
            },
        ))
    }

    fn session(&self, id: String) -> SessionName {
        SessionName {
            label: self.chat.session_label(&id),
            id,
        }
    }

    /// `at` on the view's clock: `15:48`.
    fn clock(&self, at: DateTime<Utc>) -> String {
        super::clock(at.with_timezone(&self.offset).naive_local())
    }
}

/// Every width zirc.dev resizes `file`'s image to, for a view to pick from by its pixels.
fn srcset(file: &zirc_proto::File) -> String {
    file::WIDTHS
        .map(|width| format!("{} {width}w", file::image_url(file, width)))
        .join(", ")
}

/// `n` as records count, which no conversation reaches past.
fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Keys for lines: the server's id where there is one, otherwise the time and how many
/// id-less lines came before it at that time, which stay put as lines arrive after them.
#[derive(Default)]
pub(super) struct Keys {
    last: Option<i64>,
    same: u32,
}

impl Keys {
    pub(super) fn of(&mut self, entry: &Entry) -> String {
        entry
            .id
            .clone()
            .unwrap_or_else(|| self.next(entry.time.timestamp_millis()))
    }

    fn next(&mut self, time_ms: i64) -> String {
        if self.last == Some(time_ms) {
            self.same += 1;
        } else {
            self.last = Some(time_ms);
            self.same = 0;
        }
        format!("{time_ms}:{}", self.same)
    }
}

/// Whether `blocks` name a file or a commit that links ([`super::code_ref`]).
fn names_code(blocks: &[RichBlock]) -> bool {
    let mut names = false;
    RichBlock::each_span(blocks, &mut |span| names |= span.code.is_some());
    names
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;
    use zirc_proto::card::{Action, Card, State, Who};
    use zirc_proto::{Event, Message};

    use super::*;
    use crate::chat::{Link, Options, RichSpanKind};

    fn noon() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 30, 12, 0, 0).unwrap()
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    fn feed(chat: &mut Chat, lines: &[&str]) {
        for line in lines {
            let _ = chat.receive(line.parse().unwrap());
        }
    }

    /// A chat as `me`, connected and in `#api` with alice and bob, at noon on 30 October.
    fn chat() -> Chat {
        let mut chat = Chat::new("me", "irc.zirc.test", Vec::new(), Options::default());
        chat.clock = noon;
        chat.connected(Link {
            nick: "me".into(),
            chathistory: true,
            echo: true,
            read_marker: false,
            trim: true,
            multiline: None,
            metadata: false,
        });
        feed(
            &mut chat,
            &[
                ":me!u@h JOIN #api",
                ":s 353 me = #api :me alice bob",
                ":s 366 me #api :End",
            ],
        );
        chat
    }

    /// `line` said at `time` on 30 October, with the id `id` when there is one.
    fn at(time: &str, id: Option<&str>, line: &str) -> String {
        let id = id.map(|id| format!(";msgid={id}")).unwrap_or_default();
        format!("@time=2026-10-30T{time}.000Z{id} {line}")
    }

    /// `message` from `nick` in `room`, as a line to feed.
    /// `me-mbp` is our machine, as its host says.
    fn zirc(nick: &str, room: &str, message: &Message) -> String {
        let mut line = crate::presence::line_from(nick, room, message).unwrap();
        if nick == "me-mbp" {
            line.source = Some(format!(
                "{nick}!u@{}",
                zirc_proto::names::machine_host("me")
            ));
        }
        line.to_string()
    }

    fn named(session: &str, name: &str, event: Event) -> Message {
        Message {
            name: Some(name.into()),
            ..Message::new(session, event)
        }
    }

    fn text(body: &str) -> Event {
        Event::Text { body: body.into() }
    }

    fn api(chat: &Chat) -> Vec<Line> {
        chat.lines("#api", utc(), false)
    }

    fn line<'a>(lines: &'a [Line], text: &str) -> &'a Line {
        lines
            .iter()
            .find(|line| line.text.contains(text))
            .unwrap_or_else(|| panic!("no line says {text}: {lines:#?}"))
    }

    #[test]
    fn plain_words_naming_code_link_once_the_forge_is_known() {
        let mut chat = chat();
        let said = ":alice!u@h PRIVMSG #api :broke at src/a.rs:6 in 3f2a1c0";
        feed(&mut chat, &[&at("09:00:00", Some("p1"), said)]);
        assert!(line(&api(&chat), "broke at").rich.is_empty());
        let push = named(
            "cc-1",
            "checkout",
            Event::Push {
                branch: "main".into(),
                head: "abc1234def".into(),
                forced: false,
                subjects: Vec::new(),
                paths: Vec::new(),
                base: None,
                lines: std::collections::BTreeMap::new(),
                repo: Some("https://github.com/acme/api".into()),
            },
        );
        feed(
            &mut chat,
            &[&at(
                "09:01:00",
                Some("p2"),
                &zirc("me-mbp", "#api/agents", &push),
            )],
        );
        let lines = api(&chat);
        let RichBlock::Paragraph { spans } = &line(&lines, "broke at").rich[0] else {
            panic!("a paragraph");
        };
        let linked: Vec<&str> = spans
            .iter()
            .filter(|span| span.code.is_some())
            .map(|span| span.text.as_str())
            .collect();
        assert_eq!(linked, ["src/a.rs:6", "3f2a1c0"]);
        // Words naming nothing stay plain.
        assert!(line(&lines, "broke at").text.contains("broke"));
    }

    #[test]
    fn keys_are_stable_and_unique_and_days_divide_once() {
        let mut chat = chat();
        feed(
            &mut chat,
            &[
                "@time=2026-10-29T09:00:00.000Z;msgid=y1 :alice!u@h PRIVMSG #api :yesterday",
                &at("09:00:00", None, ":alice!u@h PRIVMSG #api :one"),
                &at("09:00:00", None, ":alice!u@h PRIVMSG #api :two"),
                &at("09:01:00", Some("m3"), ":bob!u@h PRIVMSG #api :three"),
                &at("09:02:00", None, ":bob!u@h PRIVMSG #api :four"),
            ],
        );
        let lines = api(&chat);
        let keys: Vec<&str> = lines.iter().map(|line| line.key.as_str()).collect();
        let nine = Utc
            .with_ymd_and_hms(2026, 10, 30, 9, 0, 0)
            .unwrap()
            .timestamp_millis();
        let two = Utc
            .with_ymd_and_hms(2026, 10, 30, 9, 2, 0)
            .unwrap()
            .timestamp_millis();
        assert_eq!(
            keys,
            [
                "y1".to_owned(),
                format!("{nine}:0"),
                format!("{nine}:1"),
                "m3".to_owned(),
                format!("{two}:0"),
            ]
        );
        // The same lines, laid out again, keep their keys.
        let again: Vec<String> = api(&chat).into_iter().map(|line| line.key).collect();
        assert_eq!(again, keys);
        let days: Vec<Option<&str>> = lines.iter().map(|line| line.day.as_deref()).collect();
        assert_eq!(days, [Some("Yesterday"), Some("Today"), None, None, None]);
        assert_eq!(lines[1].clock, "09:00");
        assert_eq!(lines[1].time_ms, nine);
        // The first of a day starts a run; the rest of a sender's carry it on.
        let continues: Vec<bool> = lines.iter().map(|line| line.continues).collect();
        assert_eq!(continues, [false, false, true, false, true]);
        assert!(lines.iter().all(|line| line.kind == LineKind::Chat));
    }

    #[test]
    fn times_and_days_are_on_the_views_clock() {
        let mut chat = chat();
        feed(
            &mut chat,
            &["@time=2026-10-29T23:30:00.000Z :alice!u@h PRIVMSG #api :late"],
        );
        let lines = chat.lines("#api", FixedOffset::east_opt(3600).unwrap(), false);
        assert_eq!(lines[0].clock, "00:30");
        assert_eq!(lines[0].day.as_deref(), Some("Today"));
        assert_eq!(api(&chat)[0].day.as_deref(), Some("Yesterday"));
    }

    #[test]
    fn says_who_said_it_and_what_concerns_us() {
        let mut chat = chat();
        feed(
            &mut chat,
            &[
                ":s 354 me 42 h bob G bob :Bob",
                ":guest-7k2q!u@h JOIN #api * :Guest",
                &at(
                    "09:00:00",
                    Some("m1"),
                    ":alice!u@h PRIVMSG #api :hey @me, look",
                ),
                &at("09:01:00", Some("m2"), ":ME!u@h PRIVMSG #api :on it")
                    .replace(";msgid=m2", ";msgid=m2;+zirc.dev/client=desktop"),
                &at("09:02:00", Some("m3"), ":bob!u@h PRIVMSG #api :fine"),
                &at(
                    "09:03:00",
                    Some("m4"),
                    ":guest-7k2q!u@h PRIVMSG #api :hello?",
                ),
                &at("09:04:00", Some("m5"), ":carol!u@h PRIVMSG #api :drive-by"),
            ],
        );
        let lines = api(&chat);
        let alice = line(&lines, "look");
        assert!(alice.mention && !alice.mine);
        assert_eq!(alice.from, "alice");
        assert_eq!(alice.presence, Some(Presence::Here));
        let speaker = alice.speaker.as_ref().unwrap();
        assert_eq!((speaker.name.as_str(), speaker.app), ("alice", false));
        // Ours by its folded nick, whatever its case; its app shown at the start of a run.
        let mine = line(&lines, "on it");
        assert!(mine.mine && !mine.mention);
        assert_eq!(mine.sent_from.as_deref(), Some("desktop"));
        assert_eq!(line(&lines, "fine").presence, Some(Presence::Away));
        let guest = line(&lines, "hello?");
        assert!(guest.guest);
        assert_eq!(line(&lines, "drive-by").presence, Some(Presence::Gone));
        assert!(!line(&lines, "fine").guest);
    }

    #[test]
    fn plain_words_have_no_blocks_and_rich_ones_do() {
        let mut chat = chat();
        feed(
            &mut chat,
            &[
                &at(
                    "09:00:00",
                    Some("m1"),
                    ":alice!u@h PRIVMSG #api :just words",
                ),
                &at(
                    "09:01:00",
                    Some("m2"),
                    ":alice!u@h PRIVMSG #api :**bold** see https://example.com/x",
                ),
                &at("09:02:00", Some("m3"), ":alice!u@h NOTICE #api :a *notice*"),
                &at(
                    "09:03:00",
                    Some("m4"),
                    ":alice!u@h PRIVMSG #api :\x01ACTION waves\x01",
                ),
            ],
        );
        let lines = api(&chat);
        let plain = line(&lines, "just words");
        assert!(plain.rich.is_empty() && plain.more.is_none());
        let rich = line(&lines, "bold");
        assert_eq!(rich.text, "**bold** see https://example.com/x");
        assert_eq!(rich.link.as_deref(), Some("https://example.com/x"));
        let [RichBlock::Paragraph { spans }] = rich.rich.as_slice() else {
            panic!("one paragraph: {:?}", rich.rich);
        };
        assert!(spans[0].bold);
        assert!(
            spans
                .iter()
                .any(|span| matches!(&span.kind, RichSpanKind::Link { url } if url == "https://example.com/x"))
        );
        // A notice has its speaker and rich text as a message does.
        let notice = line(&lines, "notice");
        assert_eq!(notice.kind, LineKind::Notice);
        assert!(notice.speaker.is_some());
        assert!(!notice.rich.is_empty());
        let action = line(&lines, "waves");
        assert_eq!(action.kind, LineKind::Action);
        assert_eq!(action.text, "waves");
    }

    /// What `nick` said or did, `kind`, in `#api` at `time` on 30 October, as an entry put
    /// in directly: for text of several lines, which a server sends as a multiline batch,
    /// and events too long for one line.
    fn put(chat: &mut Chat, nick: &str, time: &str, id: &str, kind: crate::chat::EntryKind) {
        let at = chat.position("#api").unwrap();
        let mention = match &kind {
            crate::chat::EntryKind::Message(text) => crate::names::mentions(text, "me"),
            _ => false,
        };
        let entry = Entry {
            time: format!("2026-10-30T{time}Z").parse().unwrap(),
            id: Some(id.into()),
            from: nick.into(),
            owner: None,
            kind,
            history: false,
            mention,
            sent_from: None,
            reply_to: None,
            formatted: None,
            edited: false,
        };
        chat.conversations[at].entries.push(entry);
    }

    fn said_in_api(chat: &mut Chat, nick: &str, time: &str, id: &str, text: &str) {
        put(
            chat,
            nick,
            time,
            id,
            crate::chat::EntryKind::Message(text.into()),
        );
    }

    #[test]
    fn code_blocks_come_highlighted() {
        let mut chat = chat();
        chat.set_highlighter(Some(crate::chat::highlighted_lines));
        said_in_api(
            &mut chat,
            "alice",
            "09:00:00",
            "m1",
            "```rust\nfn main() {}\n```",
        );
        let lines = api(&chat);
        let [
            RichBlock::Code {
                language,
                lines: code,
                ..
            },
        ] = lines[0].rich.as_slice()
        else {
            panic!("a code block: {:?}", lines[0].rich);
        };
        assert_eq!(language.as_deref(), Some("rust"));
        assert_eq!(code[0].tokens[0].text, "fn");
        assert_eq!(code[0].tokens[0].role, "syntax-keyword");
    }

    #[test]
    fn long_messages_are_cut_with_one_control() {
        let mut chat = chat();
        let long: Vec<String> = (1..=12).map(|n| format!("line {n}")).collect();
        said_in_api(&mut chat, "alice", "09:00:00", "m1", &long.join("\n"));
        let first = api(&chat).remove(0);
        let more = first.more.clone().expect("a control");
        assert!(!more.open);
        assert_eq!(more.hidden, 6);
        assert_eq!(more.label, "Show 6 more lines");
        assert_eq!(more.spoken, "Show 6 more lines of alice's message");
        assert_eq!(more.code_lines, None);
        // The whole text is kept for copying, whatever shows.
        assert_eq!(first.text, long.join("\n"));
        chat.toggle_more(&first.key, more.open);
        let opened = api(&chat).remove(0);
        let more = opened.more.expect("a control");
        assert!(more.open);
        assert_eq!(more.label, "Show less");
    }

    #[test]
    fn counts_reactions_and_marks_ours() {
        let mut chat = chat();
        feed(
            &mut chat,
            &[
                &at("09:00:00", Some("m1"), ":alice!u@h PRIVMSG #api :ship it?"),
                "@+draft/react=👍;+draft/reply=m1 :bob!u@h TAGMSG #api",
                "@+draft/react=👍;+draft/reply=m1 :me!u@h TAGMSG #api",
                "@+draft/react=🎉;+draft/reply=m1 :bob!u@h TAGMSG #api",
            ],
        );
        assert_eq!(
            api(&chat)[0].reactions,
            [
                LineReaction {
                    emoji: "👍".into(),
                    count: 2,
                    mine: true
                },
                LineReaction {
                    emoji: "🎉".into(),
                    count: 1,
                    mine: false
                },
            ]
        );
    }

    #[test]
    fn an_event_offers_what_a_message_does_but_deleting() {
        let mut chat = chat();
        // An event our own machine's zircd said, as every zirc event is: a PRIVMSG the
        // server names, which a reaction names in turn.
        let intent = named(
            "cc-1",
            "checkout",
            Event::Intent {
                paths: vec!["web/src".into()],
                summary: "tidy the form".into(),
                expires: None,
                lasts: None,
            },
        );
        feed(
            &mut chat,
            &[
                &at("09:00:00", Some("e1"), &zirc("me", "#api", &intent)),
                "@+draft/react=👀;+draft/reply=e1 :bob!u@h TAGMSG #api",
                &at("09:01:00", Some("m1"), ":me!u@h PRIVMSG #api :mine"),
                &at("09:02:00", Some("m2"), ":bob!u@h PRIVMSG #api :theirs"),
                &at("09:03:00", None, ":carol!u@h JOIN #api"),
            ],
        );
        let lines = api(&chat);
        let event = line(&lines, "tidy the form");
        assert_eq!(event.kind, LineKind::Event);
        assert!(event.mine);
        assert_eq!(
            event.actions,
            LineActions {
                react: true,
                reply: true,
                copy_link: true,
                edit: false,
                delete: false,
            }
        );
        assert_eq!(
            event.reactions,
            [LineReaction {
                emoji: "👀".into(),
                count: 1,
                mine: false
            }]
        );
        assert!(line(&lines, "mine").actions.delete);
        let theirs = line(&lines, "theirs").actions;
        assert!(theirs.react && theirs.reply && theirs.copy_link && !theirs.delete);
        assert_eq!(line(&lines, "joined").actions, LineActions::default());
    }

    #[test]
    fn quotes_what_a_reply_answers_while_it_is_kept() {
        let mut chat = chat();
        let profiles: zirc_proto::profile::Profiles =
            serde_json::from_str(r#"{"alice": {"name": "Alice Liddell", "avatar": "3"}}"#).unwrap();
        chat.take_profiles(&profiles, "https://zirc.dev/");
        chat.set_picture_site(Some("https://zirc.dev".into()), Some("acme".into()));
        feed(
            &mut chat,
            &[
                &at(
                    "09:00:00",
                    Some("m1"),
                    ":alice!u@h PRIVMSG #api :the login bug",
                ),
                &at("09:01:00", Some("m2"), ":bob!u@h PRIVMSG #api :on it")
                    .replace(";msgid=m2", ";msgid=m2;+draft/reply=m1"),
                &at("09:02:00", Some("m3"), ":bob!u@h PRIVMSG #api :and that")
                    .replace(";msgid=m3", ";msgid=m3;+draft/reply=gone"),
            ],
        );
        let lines = api(&chat);
        assert_eq!(lines[0].quote, None);
        assert_eq!(
            line(&lines, "on it").quote,
            Some(LineQuote {
                id: "m1".into(),
                from: "Alice Liddell".into(),
                text: "the login bug".into(),
                face: Some(Face::Person {
                    nick: "alice".into(),
                    initials: "AL".into(),
                    hue: crate::chat::nick_degrees("alice"),
                    photo: Some("https://zirc.dev/m/acme/@alice?v=3".into()),
                }),
            })
        );
        // One no longer kept: its id alone, for "a reply to an earlier message".
        assert_eq!(
            line(&lines, "and that").quote,
            Some(LineQuote {
                id: "gone".into(),
                from: String::new(),
                text: String::new(),
                face: None,
            })
        );
    }

    #[expect(clippy::too_many_lines, reason = "one story, told step by step")]
    #[test]
    fn an_event_comes_in_parts() {
        let mut chat = chat();
        let repo = "https://github.com/acme/api";
        let sha = "abc1234def5678abc1234def5678abc1234def56";
        let commit = named(
            "cc-1",
            "checkout",
            Event::Commit {
                branch: "main".into(),
                sha: sha.into(),
                subject: "Fix login".into(),
                paths: vec!["web/src/login.ts".into()],
                repo: Some(repo.into()),
            },
        );
        let push = named(
            "cc-1",
            "checkout",
            Event::Push {
                branch: "main".into(),
                head: sha.into(),
                forced: false,
                subjects: vec!["Fix login".into()],
                paths: Vec::new(),
                base: None,
                lines: std::collections::BTreeMap::new(),
                repo: Some(repo.into()),
            },
        );
        let to = named("cc-1", "checkout", text("cc-598aa1: please rebase"));
        let intent = named(
            "cc-1",
            "checkout",
            Event::Intent {
                paths: vec!["web/src".into()],
                summary: "tidy the form".into(),
                expires: None,
                lasts: None,
            },
        );
        feed(
            &mut chat,
            &[
                &at("09:00:00", Some("e1"), &zirc("me-mbp", "#api", &commit)),
                &at("09:01:00", Some("e2"), &zirc("me-mbp", "#api", &push)),
                &at("09:02:00", Some("e3"), &zirc("me-mbp", "#api", &to)),
                &at("09:03:00", Some("e4"), &zirc("me-mbp", "#api", &intent)),
            ],
        );
        let lines = api(&chat);
        assert_eq!(lines.len(), 3, "the push folds into its commit: {lines:#?}");
        let made = &lines[0];
        assert_eq!(made.kind, LineKind::Event);
        let system = made.system.as_ref().unwrap();
        assert_eq!(system.icon, Icon::Commit);
        assert_eq!(system.glyph, "●");
        assert_eq!(system.verb, "committed and pushed to main");
        assert_eq!(system.lead, "● committed and pushed to main");
        assert_eq!(system.actor, "checkout");
        assert_eq!(
            system.session,
            Some(SessionName {
                id: "cc-1".into(),
                label: "checkout".into()
            })
        );
        // A commit's paths aren't said in its line.
        assert!(system.paths.is_empty());
        assert_eq!(made.clock, "09:00");
        assert_eq!(system.until_clock.as_deref(), Some("09:01"));
        assert!(!system.compact && !system.card);
        // The branch links to its page on the forge.
        assert!(system.verb_links.iter().any(|span| span.text == "main"
            && span.kind
                == RichSpanKind::Link {
                    url: crate::chat::branch_link(repo, "main")
                }));
        assert_eq!(system.lead_links.len(), system.verb_links.len());
        // The details link the commit, when they name it; else nothing links.
        assert!(
            system.details_links.is_empty()
                || system
                    .details_links
                    .iter()
                    .any(|span| matches!(span.kind, RichSpanKind::Link { .. }))
        );
        let speaker = made.speaker.as_ref().unwrap();
        assert_eq!(speaker.session.as_deref(), Some("cc-1"));
        assert_eq!(speaker.detail.as_deref(), Some("your agent"));
        // A post to another session says whom.
        let post = line(&lines, "please rebase");
        let system = post.system.as_ref().unwrap();
        assert_eq!(system.icon, Icon::Post);
        assert_eq!(system.lead, "");
        assert_eq!(
            system.to,
            Some(SessionName {
                id: "cc-598aa1".into(),
                label: "cc-598aa1".into()
            })
        );
        assert_eq!(system.details, "please rebase");
        assert!(system.details_links.is_empty());
        assert_eq!(system.until_clock, None);
        // Plain words, shown whole, need no blocks: its details say them.
        assert!(post.rich.is_empty() && post.more.is_none());
        let about = lines[2].system.as_ref().unwrap();
        assert_eq!(about.icon, Icon::Intent);
        assert_eq!(about.verb, "is about to change");
        assert_eq!(about.details, "tidy the form");
        assert_eq!(about.paths, ["web/src"]);
        // Its paths are listed under what it's for, which starts a line of its own.
        assert!(about.paths_listed && about.details_apart && !about.one_phrase);
        // A commit's line has no paths, and its details follow its lead.
        let made = lines[0].system.as_ref().unwrap();
        assert!(!made.paths_listed && !made.details_apart && !made.one_phrase);
    }

    #[test]
    fn a_push_of_several_commits_lists_them_apart_and_overlap_is_one_phrase() {
        let mut chat = chat();
        let push = named(
            "cc-1",
            "checkout",
            Event::Push {
                branch: "main".into(),
                head: "abc1234def".into(),
                forced: false,
                subjects: vec!["One".into(), "Two".into()],
                paths: Vec::new(),
                base: None,
                lines: std::collections::BTreeMap::new(),
                repo: None,
            },
        );
        feed(
            &mut chat,
            &[&at("09:00:00", Some("p1"), &zirc("me-mbp", "#api", &push))],
        );
        let lines = api(&chat);
        let pushed = lines[0].system.as_ref().unwrap();
        assert_eq!(pushed.verb, "pushed main to abc1234 (2 commits)");
        assert_eq!(pushed.details, "One\nTwo");
        // Lines of their own, with no paths under them: still apart from the lead.
        assert!(pushed.details_apart && !pushed.paths_listed);
    }

    #[test]
    fn a_posts_words_are_drawn_and_cut_as_a_messages() {
        let mut chat = chat();
        let steps: Vec<String> = (1..=12).map(|n| format!("- step {n}")).collect();
        let long = named("cc-1", "checkout", text(&steps.join("\n")));
        let bold = named("cc-1", "checkout", text("cc-598aa1: **please** rebase"));
        feed(
            &mut chat,
            &[
                &at("09:00:00", Some("e1"), &zirc("me-mbp", "#api", &long)),
                &at("09:01:00", Some("e2"), &zirc("me-mbp", "#api", &bold)),
            ],
        );
        let lines = api(&chat);
        let cut = &lines[0];
        assert_eq!(cut.kind, LineKind::Event);
        assert!(matches!(cut.rich.as_slice(), [RichBlock::Items { .. }]));
        let more = cut.more.as_ref().expect("a control");
        assert_eq!(more.label, "Show 6 more lines");
        assert_eq!(more.spoken, "Show 6 more lines of your agent's message");
        // Addressed to a session, its words are what follows the session.
        let to = &lines[1];
        assert_eq!(
            to.system.as_ref().and_then(|system| system.to.as_ref()),
            Some(&SessionName {
                id: "cc-598aa1".into(),
                label: "cc-598aa1".into()
            })
        );
        let [RichBlock::Paragraph { spans }] = to.rich.as_slice() else {
            panic!("one paragraph: {:?}", to.rich);
        };
        assert!(spans[0].bold && spans[0].text == "please");
    }

    fn ci_landing(text: &str) -> Message {
        Message::new(
            "ci",
            Event::Custom {
                kind: zirc_proto::ci::LANDING_KIND.into(),
                data: serde_json::json!({
                    "session": "cc-1",
                    "landing": "monorepo/771",
                    "build": "771",
                    "url": "https://buildkite.com/zirc/monorepo/builds/771",
                    "branch": "fix-login",
                }),
                text: text.into(),
            },
        )
    }

    #[test]
    fn a_reply_to_a_ci_line_quotes_what_the_line_says() {
        let mut chat = chat();
        let failed = ci_landing("cc-1: build 771 failed: published code");
        put(
            &mut chat,
            "zirc-ci",
            "09:00:00",
            "l1",
            crate::chat::EntryKind::Zirc(failed),
        );
        feed(
            &mut chat,
            &[&at(
                "09:01:00",
                Some("m2"),
                ":bob!u@h PRIVMSG #api :two flaky tests",
            )
            .replace(";msgid=m2", ";msgid=m2;+draft/reply=l1")],
        );
        let lines = api(&chat);
        let quote = line(&lines, "two flaky tests").quote.clone().unwrap();
        assert_eq!(
            (quote.id.as_str(), quote.text.as_str()),
            ("l1", "cc-1: build 771 failed: published code")
        );
    }

    #[test]
    fn a_landing_is_one_line_with_its_outcome() {
        let mut chat = chat();
        let landed = ci_landing("cc-1: landed fix-login as abc1234 (build 771)");
        put(
            &mut chat,
            "zirc-ci",
            "09:00:00",
            "l1",
            crate::chat::EntryKind::Zirc(landed),
        );
        let lines = api(&chat);
        let landed = &lines[0];
        assert_eq!(landed.kind, LineKind::Landing);
        assert!(!landed.mine);
        assert_eq!(
            landed.link.as_deref(),
            Some("https://buildkite.com/zirc/monorepo/builds/771")
        );
        assert!(landed.text.contains("fix-login") && landed.text.contains("build 771"));
        let system = landed.system.as_ref().unwrap();
        assert_eq!(system.outcome, Some(Icon::Landed));
        assert!(system.card);
        assert_eq!(system.buttons, None);
        assert_eq!(system.card_version, None);
        assert_eq!(system.session.as_ref().map(|s| s.id.as_str()), Some("cc-1"));
        // Its build links to its page.
        let [RichBlock::Paragraph { spans }, ..] = landed.rich.as_slice() else {
            panic!("the card's line: {:?}", landed.rich);
        };
        assert!(
            spans
                .iter()
                .any(|span| span.text == "build 771"
                    && matches!(span.kind, RichSpanKind::Link { .. }))
        );
        assert!(landed.speaker.as_ref().is_some_and(|speaker| speaker.app));
    }

    fn card(actions: Vec<Action>) -> Card {
        Card {
            id: "deploy/42".into(),
            title: "web · deploy 42".into(),
            state: State::Pending,
            about: None,
            fields: Vec::new(),
            links: Vec::new(),
            actions,
            note: None,
        }
    }

    fn action(id: &str, who: Who) -> Action {
        Action {
            id: id.into(),
            label: id.to_uppercase(),
            who,
            confirm: Some(format!("{id}?")),
        }
    }

    #[test]
    fn a_card_shows_us_the_buttons_we_may_press() {
        let mut chat = chat();
        let card = card(vec![
            action("approve", Who::Members),
            action("cancel", Who::Owners),
        ]);
        let posted = Message::new(
            "deploy",
            Event::Custom {
                kind: "x-zirc/deploy".into(),
                data: card.as_data(),
                text: "web · deploy 42 pending".into(),
            },
        );
        put(
            &mut chat,
            "zirc-deploy",
            "09:00:00",
            "c1",
            crate::chat::EntryKind::Zirc(posted),
        );
        let lines = api(&chat);
        let system = lines[0].system.as_ref().unwrap();
        assert!(system.card);
        let version = system.card_version.clone().expect("a card's version");
        assert_eq!(
            system.buttons,
            Some(CardButtons {
                sender: "zirc-deploy".into(),
                card: "deploy/42".into(),
                key: version,
                actions: vec![CardAction {
                    id: "approve".into(),
                    label: "APPROVE".into(),
                    confirm: Some("approve?".into()),
                }],
            })
        );
        assert_eq!(system.outcome, None);
    }

    #[test]
    fn people_coming_and_going_are_one_quiet_line() {
        let mut chat = chat();
        feed(
            &mut chat,
            &[
                &at("09:00:00", None, ":carol!u@h JOIN #api"),
                &at("09:00:00", None, ":dave!u@h JOIN #api"),
                &at("09:05:00", None, ":alice!u@h PART #api :bye"),
            ],
        );
        let lines = api(&chat);
        assert_eq!(lines.len(), 1, "{lines:#?}");
        let came = &lines[0];
        assert_eq!(came.kind, LineKind::Membership);
        assert_eq!(came.text, "carol and dave joined, alice left");
        assert_eq!(came.from, "");
        assert!(came.speaker.is_none() && came.presence.is_none());
        let system = came.system.as_ref().unwrap();
        assert!(system.compact);
        assert_eq!(system.icon, Icon::Join);
        assert_eq!(system.verb, came.text);
        let nine = Utc
            .with_ymd_and_hms(2026, 10, 30, 9, 0, 0)
            .unwrap()
            .timestamp_millis();
        assert_eq!(came.key, format!("{nine}:0"));
    }

    fn shared(mime: &str, url: &str, size: (Option<u32>, Option<u32>)) -> Message {
        Message::new(
            "desktop",
            Event::File(zirc_proto::File {
                url: url.into(),
                name: "IMG_2041.jpeg".into(),
                mime: mime.into(),
                size: 2_345_678,
                width: size.0,
                height: size.1,
                duration_ms: None,
            }),
        )
    }

    #[test]
    fn a_hosted_file_is_drawn_and_another_only_said() {
        let mut chat = chat();
        let hosted = "https://zirc.dev/m/acme/7Hq2xV9k";
        feed(
            &mut chat,
            &[
                &at(
                    "09:00:00",
                    Some("f1"),
                    &zirc(
                        "alice",
                        "#api",
                        &shared("image/jpeg", hosted, (Some(4032), Some(3024))),
                    ),
                ),
                &at(
                    "09:01:00",
                    Some("f2"),
                    &zirc("alice", "#api", &shared("video/mp4", hosted, (None, None))),
                ),
                &at(
                    "09:02:00",
                    Some("f3"),
                    &zirc("alice", "#api", &shared("text/plain", hosted, (None, None))),
                ),
                &at(
                    "09:03:00",
                    Some("f4"),
                    &zirc(
                        "alice",
                        "#api",
                        &shared("image/png", "https://evil.example/x", (Some(10), Some(10))),
                    ),
                ),
            ],
        );
        // With no web app said, nothing is loaded.
        assert!(api(&chat).iter().all(|line| line.kind == LineKind::Note));
        chat.set_picture_site(Some("https://zirc.dev".into()), Some("acme".into()));
        let lines = api(&chat);
        let image = &lines[0];
        assert_eq!(image.kind, LineKind::File);
        assert!(image.speaker.is_some());
        let file = image.file.as_ref().unwrap();
        assert_eq!(file.shape, FileShape::Image);
        assert_eq!(file.url.as_deref(), Some(hosted));
        assert_eq!((file.width, file.height), (Some(320), Some(240)));
        assert_eq!(file.size, 2_345_678);
        assert_eq!(file.size_text, "2.3 MB");
        assert_eq!(file.mime, "image/jpeg");
        let srcset = file.srcset.as_deref().unwrap();
        assert!(
            srcset.starts_with(&format!("{hosted}?w=64 64w, ")),
            "{srcset}"
        );
        assert_eq!(file.poster, None);
        let video = lines[1].file.as_ref().unwrap();
        assert_eq!(video.shape, FileShape::Video);
        assert_eq!(video.poster, Some(format!("{hosted}?w=1280")));
        assert_eq!((video.width, video.height), (None, None));
        let text = lines[2].file.as_ref().unwrap();
        assert_eq!(text.shape, FileShape::Text);
        assert_eq!((text.srcset.as_ref(), text.poster.as_ref()), (None, None));
        let elsewhere = &lines[3];
        assert_eq!(elsewhere.kind, LineKind::Note);
        assert_eq!(elsewhere.file, None);
        assert_eq!(
            elsewhere.text,
            "alice shared IMG_2041.jpeg (2.3 MB) from elsewhere, not shown"
        );
    }

    #[test]
    fn a_self_hosted_organisations_files_are_drawn_from_its_own_server() {
        let mut chat = chat();
        let own = "https://irc.acme.com/m/7Hq2xV9k";
        let web = "https://zirc.dev/m/acme/7Hq2xV9k";
        feed(
            &mut chat,
            &[
                &at(
                    "09:00:00",
                    Some("f1"),
                    &zirc("alice", "#api", &shared("video/mp4", own, (None, None))),
                ),
                &at(
                    "09:01:00",
                    Some("f2"),
                    &zirc(
                        "alice",
                        "#api",
                        &shared("image/png", web, (Some(10), Some(10))),
                    ),
                ),
            ],
        );
        chat.set_picture_site(Some("https://zirc.dev".into()), Some("acme".into()));
        chat.set_files_site(Some("https://irc.acme.com".into()));
        let lines = api(&chat);
        assert_eq!(lines[0].kind, LineKind::File);
        assert_eq!(lines[0].file.as_ref().unwrap().url.as_deref(), Some(own));
        // Its files are its server's: one at the web app isn't its own.
        assert_eq!(lines[1].kind, LineKind::Note);
        // People's pictures still come from the web app.
        assert_eq!(chat.picture_web(), Some("https://zirc.dev"));
    }

    #[test]
    fn people_only_keeps_what_people_said() {
        let mut chat = chat();
        let post = named("cc-1", "checkout", text("done: the form"));
        feed(
            &mut chat,
            &[
                &at("09:00:00", Some("m1"), ":alice!u@h PRIVMSG #api :hi"),
                &at("09:01:00", Some("e1"), &zirc("me-mbp", "#api", &post)),
            ],
        );
        assert_eq!(chat.lines("#api", utc(), false).len(), 2);
        let people: Vec<String> = chat
            .lines("#api", utc(), true)
            .into_iter()
            .map(|line| line.text)
            .collect();
        assert_eq!(people, ["hi"]);
        assert!(chat.lines("#nowhere", utc(), false).is_empty());
    }

    #[test]
    fn an_agents_lines_carry_their_rooms_and_break_runs_between_them() {
        let mut chat = chat();
        feed(
            &mut chat,
            &[":me-mbp!u@machine/me JOIN #api", ":me!u@h JOIN #api/agents"],
        );
        let checkout = |body: &str| named("cc-1", "checkout", text(body));
        feed(
            &mut chat,
            &[
                &at(
                    "10:00:00",
                    Some("a1"),
                    &zirc("me-mbp", "#api/agents", &checkout("taking: the login page")),
                ),
                &at(
                    "10:00:30",
                    Some("a2"),
                    &zirc("me-mbp", "#api", &checkout("first in api")),
                ),
                &at(
                    "10:01:00",
                    Some("a3"),
                    &zirc("me-mbp", "#api", &checkout("second in api")),
                ),
                &at(
                    "10:02:00",
                    Some("a4"),
                    &zirc("me-mbp", "#api/agents", &checkout("back in agents")),
                ),
            ],
        );
        let lines = chat.agent_lines("me-mbp/cc-1", utc());
        let rooms: Vec<(Option<&str>, bool)> = lines
            .iter()
            .map(|line| (line.room.as_deref(), line.continues))
            .collect();
        assert_eq!(
            rooms,
            [
                (Some("#api/agents"), false),
                (Some("#api"), false),
                (Some("#api"), true),
                (Some("#api/agents"), false),
            ]
        );
        assert_eq!(lines[0].day.as_deref(), Some("Today"));
        // Its own lines leave whose agent it is to the header.
        assert!(lines.iter().all(|line| {
            line.speaker
                .as_ref()
                .is_some_and(|speaker| speaker.detail.is_none() && speaker.name == "checkout")
        }));
        // In a channel the same lines say whose agent it is.
        assert!(api(&chat).iter().any(|line| {
            line.speaker
                .as_ref()
                .is_some_and(|speaker| speaker.detail.is_some())
        }));
        assert!(chat.agent_lines("#api", utc()).is_empty());
    }

    #[test]
    fn offers_editing_our_own_words_for_twenty_minutes_and_marks_them_edited() {
        let mut chat = chat();
        feed(
            &mut chat,
            &[
                &at("11:30:00", Some("m1"), ":me!u@h PRIVMSG #api :too old"),
                &at("11:45:00", Some("m2"), ":me!u@h PRIVMSG #api :teh fix"),
                &at("11:46:00", Some("m3"), ":bob!u@h PRIVMSG #api :theirs"),
                &at("11:47:00", Some("m4"), ":me!u@h NOTICE #api :a notice"),
                &at(
                    "11:50:00",
                    Some("m5"),
                    "@+draft/edit=m2 :me!u@h PRIVMSG #api :the fix",
                )
                .replace(" @", ";"),
            ],
        );
        let lines = api(&chat);
        assert!(!line(&lines, "too old").actions.edit);
        assert!(line(&lines, "too old").actions.delete);
        let fixed = line(&lines, "the fix");
        assert!(fixed.actions.edit && fixed.edited);
        assert!(!line(&lines, "theirs").actions.edit);
        assert!(!line(&lines, "a notice").actions.edit);
        assert_eq!(lines.iter().filter(|l| l.text.contains("fix")).count(), 1);
    }
}
