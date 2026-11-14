//! A conversation laid out as rows of the scrollback: the lines zirc-core says it has
//! ([`items`]: days, runs, events, comings and goings), and long text wrapped to the width.
//! As zirc's other apps show them, a run starts with a header, the nick and then the time,
//! and its messages follow under it; compact, as IRC clients show them, each row has a time
//! and a nick in front, the nick only at the start of a run. The UI only has to colour them
//! in.

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, Utc};
use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};

use zirc_core::chat::file::size_text;

use crate::hyperlink::{Link, Links, fragments, label};
use zirc_core::chat::{
    Avatar, CellAlign, Chat, Conversation, Entry, EntryKind, GUEST_PILL, Icon, Item, Landing,
    Parts, SenderKind, SpanKind, Speaker, day_label, people_only, sender_kind, strip_formatting,
    tables_in,
};
use zirc_proto::card::Card;

/// Width of the time in front of a row: `12:04`.
pub const TIME_WIDTH: usize = 5;
/// The narrowest and widest the nick column gets.
const NICK_WIDTH: (usize, usize) = (4, 16);
/// The narrowest text column, however narrow the terminal.
const MIN_TEXT_WIDTH: usize = 10;
/// How far in the text goes under a run's header, leaving room for a mark (`◆`, `→`) or
/// the bar of the message picked.
pub const INDENT: usize = 2;

/// Which time zone to show times in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    /// The system's.
    Local,
    /// A fixed offset, for tests.
    Fixed(FixedOffset),
}

impl Zone {
    /// `time` on the wall clock.
    #[must_use]
    pub fn wall(self, time: DateTime<Utc>) -> NaiveDateTime {
        match self {
            Self::Local => time.with_timezone(&chrono::Local).naive_local(),
            Self::Fixed(offset) => time.with_timezone(&offset).naive_local(),
        }
    }

    /// The day `time` falls on.
    #[must_use]
    pub fn date(self, time: DateTime<Utc>) -> NaiveDate {
        self.wall(time).date()
    }
}

/// One row of the scrollback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A day starts: `Today`, `Yesterday`, or its date.
    Divider(String),
    /// What hadn't been read when the conversation was shown starts here.
    New,
    /// A card's buttons we may press (`/press`), by their labels: `Retry`, `Cancel`.
    Buttons {
        /// The card's message.
        id: Option<String>,
        /// The buttons' labels, in order.
        labels: Vec<String>,
    },
    /// A system row drawn in one line: someone coming or going, or any event in the IRC
    /// display mode. Its icon, who in bold and what they did, then when, dim; what it's
    /// about follows in [`Tone::Status`] rows when it wraps. Any other event is a message:
    /// a [`Row::Header`], unless it carries on a run, then a [`Tone::Lead`] row, zirc-chat's
    /// `Parts::lead`, [`Tone::Message`] rows for what it was, and a [`Tone::Path`] row.
    System {
        /// When, `12:04`.
        time: String,
        /// What sort it is.
        icon: Icon,
        /// Who: `dana's agent`, a nick, or nothing.
        actor: String,
        /// What they did, after who, on a one-line row; empty on an event's header.
        verb: String,
        /// The id of the event, when it can be reacted to.
        id: Option<String>,
        /// When an agent session did it, its mark and whose agent it is.
        agent: Option<Agent>,
        /// The links on its line: the branch and commit it names, to their pages.
        links: Vec<Link>,
        /// Whether it mentions us: zirc-chat's `Item::mentions_me`.
        mention: bool,
    },
    /// Part of an entry.
    Text {
        /// The time, on an entry's first row when it's worth showing; empty otherwise.
        time: String,
        /// What goes in the nick column: a nick, a marker such as `→`, or nothing.
        nick: String,
        /// The text on this row.
        text: String,
        /// How it reads.
        tone: Tone,
        /// Whether the entry mentions us.
        mention: bool,
        /// Which app a message was sent from, `· desktop`, on its first row when zirc-core
        /// says to show it; empty otherwise.
        hint: String,
        /// The id of the message the row is part of, when it's one that can be reacted to.
        id: Option<String>,
        /// The links the row shows, to draw as hyperlinks.
        links: Vec<Link>,
    },
    /// The start of a run of what someone said, as zirc's other apps show it: who, and when.
    Header {
        /// Who said it.
        nick: String,
        /// When, `12:04`.
        time: String,
        /// How it reads: a message, or a notice.
        tone: Tone,
        /// Which app it was sent from, `· desktop`, when zirc-core says to show it.
        hint: String,
        /// The id of its first message.
        id: Option<String>,
        /// What's marked after the nick, zirc-chat's ([`Chat::name_pill`]): `GUEST` for a
        /// guest, `APP` for an app.
        pill: Option<&'static str>,
        /// When an agent session said it, its mark and whose agent it is.
        agent: Option<Agent>,
    },
    /// The reactions to the message `id`, or as many as fit on a row, under it.
    Reactions {
        /// The message.
        id: String,
        /// The reactions, in the order they were first given.
        chips: Vec<Chip>,
    },
}

/// One reaction to a message, as zirc-chat counts it ([`Conversation::chips`]).
pub use zirc_core::chat::ReactionChip as Chip;

/// A chip as drawn, padded: ` 🎉 2 `.
#[must_use]
pub fn chip_text(chip: &Chip) -> String {
    format!(" {} {} ", chip.emoji, chip.count)
}

/// The gap between two chips.
pub const CHIP_GAP: usize = 1;

/// Which of `chips`, drawn from the start of the text column, is at `column` within it.
#[must_use]
pub fn chip_at(chips: &[Chip], column: usize) -> Option<&Chip> {
    let mut start = 0;
    for chip in chips {
        let end = start + chip_text(chip).width();
        if (start..end).contains(&column) {
            return Some(chip);
        }
        start = end + CHIP_GAP;
    }
    None
}

/// Where the text column starts in a row of `conversation`'s scrollback, `compact` or not.
#[must_use]
pub fn text_column(conversation: &Conversation, compact: bool) -> usize {
    column(nick_width(conversation), compact)
}

/// Where the text column starts, after a nick column `nick_width` wide when `compact`.
fn column(nick_width: usize, compact: bool) -> usize {
    if compact {
        TIME_WIDTH + 1 + nick_width + 1
    } else {
        INDENT
    }
}

/// What an item of the conversation comes to, before it's laid out in rows.
struct Piece {
    /// The time, `12:04`.
    time: String,
    /// Who said it, or a mark, as `→`, for what nobody said.
    nick: String,
    text: String,
    tone: Tone,
    mention: bool,
    hint: String,
    /// What goes dim at the end of its first row: the session an app's post is about.
    after: String,
    id: Option<String>,
    /// Whether it continues the run before it.
    continues: bool,
    /// Whether someone said it, rather than it being an event or a coming and going.
    said: bool,
    /// What it answers, as quoted above it.
    quote: Option<String>,
    /// What's marked after the nick: see [`Row::Header`].
    pill: Option<&'static str>,
    /// The labels of the links in [`Piece::text`], with their addresses.
    links: Vec<(String, Option<String>)>,
    /// When an agent session said it: see [`Row::Header`].
    agent: Option<Agent>,
}

/// An agent session heading a row, as zirc-chat's [`Speaker`] says, where other apps
/// draw its owner's photo with a robot badge: a mark for its tool in its own colour, and
/// whose agent it is after its name, `mattias's agent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    /// One cell for its tool: see [`zirc_core::chat::tool_mark`].
    pub mark: &'static str,
    /// Its colour's hue in degrees, below 360, the same every time for the same session.
    pub hue: u16,
    /// What follows its name, muted: `mattias's agent`; empty for the machine itself.
    pub detail: String,
}

/// The columns an agent's mark takes in front of a system row's icon, with its space.
pub const AGENT_MARK_WIDTH: usize = 2;

/// How a row heads who it's from, by [`Speaker`]: an agent session by its name with its
/// [`Agent`] mark, a person or an app by their name; and whether it's an app, tagged
/// [`APP_TAG`] after it.
fn head(speaker: Speaker) -> (String, Option<Agent>, bool) {
    let app = speaker.is_app;
    match speaker.avatar {
        avatar @ Avatar::Agent { .. } => (
            speaker.name,
            agent_mark(&avatar).map(|agent| Agent {
                detail: speaker.detail.unwrap_or_default(),
                ..agent
            }),
            app,
        ),
        _ => (speaker.name, None, app),
    }
}

/// An agent's avatar as its mark, with nothing after its name; `None` for anyone else's.
fn agent_mark(avatar: &Avatar) -> Option<Agent> {
    match avatar {
        Avatar::Agent { tool, hue, .. } => Some(Agent {
            mark: zirc_core::chat::tool_mark(tool.as_deref()),
            hue: *hue,
            detail: String::new(),
        }),
        _ => None,
    }
}

/// What's marked after the nick of what was said, `said`, as zirc-chat says
/// ([`Chat::name_pill`]): a guest as outside, an app by its tag, `CI APP`.
fn pill(chat: &Chat, said: Option<&Entry>, app: bool) -> Option<&'static str> {
    match said {
        Some(entry) => chat.name_pill(&entry.from, app),
        None => app.then_some(zirc_core::chat::APP_TAG),
    }
}

/// How a row reads, for colouring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// A message.
    Message,
    /// An action.
    Action,
    /// A notice.
    Notice,
    /// An app's post, such as CI's, with its outcome's icon in front.
    App(Option<Icon>),
    /// An event's lead, zirc-chat's `Parts::lead`: its marker and what it did, in its
    /// icon's colour.
    Lead(Icon),
    /// What a one-line system row is about, wrapped.
    Status,
    /// The paths a system row is about.
    Path,
    /// This client speaking.
    Info,
    /// The message a reply answers, quoted above it.
    Quote,
}

/// Who's who, for [`layout`].
#[derive(Clone, Copy)]
pub struct Who<'a> {
    /// The chat the conversation is in, which says who's who: us, guests, names, agent
    /// sessions, speakers, and which of a card's buttons we're shown.
    pub chat: &'a Chat,
    /// What came of our pressing one of the card from the sender, if anything, until the
    /// card changes.
    pub pressed: &'a dyn Pressed,
}

/// What came of our pressing one of a card's buttons, as [`Who::pressed`] says.
pub trait Pressed {
    /// What came of our pressing one of `card`'s from `sender`, if anything, until the
    /// card changes.
    fn pressed(&self, sender: &str, card: &Card) -> Option<String>;
}

impl<F: Fn(&str, &Card) -> Option<String>> Pressed for F {
    fn pressed(&self, sender: &str, card: &Card) -> Option<String> {
        self(sender, card)
    }
}

/// The rows of `conversation` at `width` columns, `today` being the date in `zone`, for us,
/// `me`: `compact`, IRC's way, or as zirc's other apps show them. The "New" divider goes
/// above the message `divider`.
#[must_use]
pub fn layout(
    conversation: &Conversation,
    who: Who<'_>,
    divider: Option<&str>,
    width: usize,
    (zone, today): (Zone, NaiveDate),
    (compact, links, people): (bool, Links, bool),
) -> Vec<Row> {
    let look = Look::new(
        conversation,
        who,
        divider,
        width,
        (zone, today),
        (compact, links),
    );
    let items = shown(who.chat, conversation, zone, people);
    (0..items.len())
        .flat_map(|at| item_rows(&look, &items[at], at.checked_sub(1).map(|i| &items[i])))
        .collect()
}

/// Everything a conversation's items are laid out by, but the items: see [`layout`].
#[derive(Clone, Copy)]
pub struct Look<'a> {
    conversation: &'a Conversation,
    who: Who<'a>,
    divider: Option<&'a str>,
    /// How wide the text column is.
    pub text_width: usize,
    /// How wide the nick column is, compact.
    nick_width: usize,
    zone: Zone,
    today: NaiveDate,
    compact: bool,
    links: Links,
}

impl<'a> Look<'a> {
    /// `conversation` laid out as [`layout`] says.
    #[must_use]
    pub fn new(
        conversation: &'a Conversation,
        who: Who<'a>,
        divider: Option<&'a str>,
        width: usize,
        (zone, today): (Zone, NaiveDate),
        (compact, links): (bool, Links),
    ) -> Self {
        let nick_width = nick_width(conversation);
        let text_width = width
            .saturating_sub(column(nick_width, compact))
            .max(MIN_TEXT_WIDTH);
        Self {
            conversation,
            who,
            divider,
            text_width,
            nick_width,
            zone,
            today,
            compact,
            links,
        }
    }

    /// The minute `item` shows, `12:04`, if it has a time.
    fn minute(&self, item: &Item<'_>) -> Option<String> {
        item.time()
            .map(|at| self.zone.wall(at).format("%H:%M").to_string())
    }
}

/// What `conversation`, in `chat`, shows, its items by day in `zone`: only what people
/// say, and whatever mentions us, when `people` is all that's wanted. Each is laid out
/// by [`item_rows`].
#[must_use]
pub fn items<'a>(
    chat: &Chat,
    conversation: &'a Conversation,
    zone: Zone,
    people: bool,
) -> Vec<Item<'a>> {
    shown(chat, conversation, zone, people)
}

/// A number that changes whenever anything [`item_rows`] lays `item`, after `before`, out
/// by does, besides what [`Look`] holds and the names [`Who`] gives: what it is, the minute
/// before it, whether the "New" divider is above it, what it answers, its reactions and what
/// came of pressing its card.
#[must_use]
pub fn fingerprint(look: &Look<'_>, item: &Item<'_>, before: Option<&Item<'_>>) -> u64 {
    use std::fmt::Write as _;
    use std::hash::Hasher as _;
    /// Hashes what's written to it, so nothing's put in a string first.
    struct Hashing(std::collections::hash_map::DefaultHasher);
    impl std::fmt::Write for Hashing {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            self.0.write(text.as_bytes());
            Ok(())
        }
    }
    let mut hashing = Hashing(std::collections::hash_map::DefaultHasher::new());
    let entry = entry_of(item);
    let id = entry.and_then(|entry| entry.id.as_deref());
    let quote = match item {
        Item::Message { entry, .. } | Item::Notice { entry, .. } | Item::Action { entry, .. } => {
            look.who.chat.quote(look.conversation, entry)
        }
        _ => None,
    };
    let chips = id.map(|id| look.conversation.chips(id, look.who.chat.me()));
    let pressed = match item {
        Item::Landing(landing) => landing
            .card
            .as_ref()
            .and_then(|card| look.who.pressed.pressed(&landing.entry.from, card)),
        _ => None,
    };
    let new = look.divider.is_some_and(|id| holds(item, id));
    let _ = write!(
        hashing,
        "{item:?}{:?}{new}{quote:?}{chips:?}{pressed:?}",
        before.and_then(|before| look.minute(before)),
    );
    hashing.0.finish()
}

/// The rows of `item`, which comes after `before`, laid out as `look` says.
#[must_use]
pub fn item_rows(look: &Look<'_>, item: &Item<'_>, before: Option<&Item<'_>>) -> Vec<Row> {
    let Look {
        conversation,
        who,
        divider,
        text_width,
        nick_width,
        compact,
        links,
        ..
    } = *look;
    let mut rows = Vec::new();
    let Some(at) = item.time() else {
        if let Item::Day(date) = item {
            rows.push(Row::Divider(day_label(*date, look.today)));
        }
        return rows;
    };
    // The minute the line before showed at, to leave out repeats within a run.
    let minute = before.and_then(|before| look.minute(before));
    if divider.is_some_and(|id| holds(item, id)) {
        rows.push(Row::New);
    }
    let shown = look.zone.wall(at).format("%H:%M").to_string();
    let entry = entry_of(item);
    let id = entry.and_then(|entry| entry.id.clone());
    let head = entry.map(|entry| head(who.chat.speaker(entry)));
    if let Some(parts) = system_parts(item) {
        // A post addressed to an agent session says to whom first, by its name.
        let parts = who.chat.addressed_parts(parts);
        system_rows(
            &mut rows,
            &parts,
            &shown,
            id.as_deref(),
            compact,
            (text_width, &|kind| who.chat.kind_label(kind), links),
            (
                head.as_ref().and_then(|head| head.1.clone()),
                pill(who.chat, entry, head.is_some_and(|head| head.2)),
                runs_on(item),
            ),
        );
        let first = usize::from(matches!(rows.first(), Some(Row::New)));
        mark_mentions(&mut rows[first..], item.mentions_me(conversation));
        return rows;
    }
    let quote = match item {
        Item::Message { entry, .. } | Item::Notice { entry, .. } | Item::Action { entry, .. } => {
            who.chat.quote(conversation, entry)
        }
        _ => None,
    };
    let said = matches!(
        item,
        Item::Message { .. } | Item::Notice { .. } | Item::File { .. } | Item::Landing(_)
    );
    let (nick, text, tone, continues) = columns(who.chat, item, compact);
    // A corrected message says so after its words, as in every zirc app.
    let text = if item.said_entry().is_some_and(|entry| entry.edited) {
        format!("{text} {}", zirc_core::chat::EDITED)
    } else {
        text
    };
    let (text, links) = linked(item, text, links, who, &conversation.name);
    // A reply names who's speaking under its quote, even within a run. The New divider
    // doesn't break a run, as in every zirc app: one header, not the same one twice.
    let continues = continues && quote.is_none();
    // Compact, the time's left out within a run when it's the same minute.
    let time = if compact && continues && minute.as_ref() == Some(&shown) {
        String::new()
    } else {
        shown
    };
    // Heading a run as zirc's other apps do, by who it's from; compact, by nick.
    let (nick, agent, app) = match head {
        _ if continues => (String::new(), None, false),
        _ if compact => (fit(&nick, nick_width), None, false),
        Some(head) if said => head,
        _ => (nick, None, false),
    };
    let (hint, after) = hints(item);
    let piece = Piece {
        time,
        nick,
        text,
        tone,
        mention: entry.is_some_and(|entry| conversation.highlights(entry)),
        hint,
        after,
        id: id.clone().filter(|_| !matches!(item, Item::Note { .. })),
        continues,
        said,
        quote: quote.map(|quote| fit(&quote.line(), text_width)),
        links,
        pill: pill(who.chat, entry.filter(|_| said), app),
        agent,
    };
    if compact {
        compact_rows(&mut rows, &piece, text_width);
    } else {
        standard_rows(&mut rows, &piece, text_width);
    }
    rows.extend(under(conversation, item, who, &piece, text_width));
    rows
}

/// The id of what `item` came from, if it has one.
#[must_use]
pub fn entry_id<'a>(item: &Item<'a>) -> Option<&'a str> {
    entry_of(item).and_then(|entry| entry.id.as_deref())
}

/// The id of the message `item` is, as its rows carry it, when it can be reacted to.
#[must_use]
pub fn item_id<'a>(item: &Item<'a>) -> Option<&'a str> {
    entry_id(item).filter(|_| !matches!(item, Item::Note { .. }))
}

/// `text`, what `item` says, with the agent sessions it names by their names (`session`)
/// and its addresses as their labels where the terminal shows `links` so; and those
/// labels with where they go: see [`crate::hyperlink`].
fn linked(
    item: &Item<'_>,
    text: String,
    links: Links,
    who: Who<'_>,
    conversation: &str,
) -> (String, Vec<(String, Option<String>)>) {
    match item {
        Item::Message { .. } | Item::Notice { .. } | Item::Action { .. } => {
            // Files and commits it names link into the editor or to the forge, as zirc-chat
            // says, where the terminal can.
            let code = if links == Links::Labelled {
                who.chat.code_links(conversation, &text)
            } else {
                Vec::new()
            };
            let (text, linked) = label(&text, links, &|kind| who.chat.kind_label(kind), &code);
            (lined_up(&text), linked)
        }
        // A landing's words link where zirc-chat says, its build, branch and commit to their
        // pages, where the terminal can; a card's say what came of our last press of it.
        Item::Landing(landing) => {
            let text = pressed(landing, text, who);
            let linked = if links == Links::Labelled {
                landing
                    .plain_links()
                    .into_iter()
                    .map(|(word, url)| (word, Some(url)))
                    .collect()
            } else {
                Vec::new()
            };
            (text, linked)
        }
        _ => (text, Vec::new()),
    }
}

/// `text` with each Markdown table in it laid out in columns, as zirc's other apps draw it
/// as a grid: each column as wide as its widest cell, lined up as its `:--`, `:-:` or `--:`
/// says, `│` between them and `─┼─` under the header. Cells keep what was typed in them.
#[must_use]
pub fn lined_up(text: &str) -> String {
    let mut text = text.to_owned();
    for table in tables_in(&text).into_iter().rev() {
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        let cell = |row: &[String], at: usize| row.get(at).map_or("", String::as_str).to_owned();
        let widths: Vec<usize> = (0..columns)
            .map(|at| {
                table
                    .rows
                    .iter()
                    .map(|row| cell(row, at).width())
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let line = |row: &[String]| {
            let cells: Vec<String> = widths
                .iter()
                .enumerate()
                .map(|(at, &width)| {
                    let words = cell(row, at);
                    let room = width.saturating_sub(words.width());
                    let before = match table.align.get(at).copied().unwrap_or_default() {
                        CellAlign::Start => 0,
                        CellAlign::Center => room / 2,
                        CellAlign::End => room,
                    };
                    format!("{}{words}{}", " ".repeat(before), " ".repeat(room - before))
                })
                .collect();
            cells.join(" │ ").trim_end().to_owned()
        };
        let mut lines: Vec<String> = table.rows.iter().map(|row| line(row)).collect();
        let rule: Vec<String> = widths.iter().map(|&width| "─".repeat(width)).collect();
        lines.insert(1.min(lines.len()), rule.join("─┼─"));
        text.replace_range(table.range, &lines.join("\n"));
    }
    text
}

/// `text`, what `landing` says, with what came of our last press of its card after its
/// first line, `web · deploy 42 failed · Retrying smoke tests`, until the card changes.
fn pressed(landing: &Landing<'_>, mut text: String, who: Who<'_>) -> String {
    let Some(result) = landing
        .card
        .as_ref()
        .and_then(|card| who.pressed.pressed(&landing.entry.from, card))
    else {
        return text;
    };
    let end = text.find('\n').unwrap_or(text.len());
    text.insert_str(end, &format!(" · {result}"));
    text
}

/// What goes under `item`, laid out as `piece` in `conversation`: its card's buttons,
/// then its reactions, `text_width` wide.
fn under(
    conversation: &Conversation,
    item: &Item<'_>,
    who: Who<'_>,
    piece: &Piece,
    text_width: usize,
) -> Vec<Row> {
    let id = piece.id.as_ref();
    let mut rows: Vec<Row> = card_buttons(item, who, id).into_iter().collect();
    if let Some(id) = id {
        rows.extend(reaction_rows(conversation, id, who.chat.me(), text_width));
    }
    rows
}

/// The row of the buttons we may press on `item`'s card, if it has any: drawn apart from
/// its text, as every app draws a card's buttons.
fn card_buttons(item: &Item<'_>, who: Who<'_>, id: Option<&String>) -> Option<Row> {
    let Item::Landing(landing) = item else {
        return None;
    };
    let card = landing.card.as_ref()?;
    let labels: Vec<String> = who
        .chat
        .card_buttons(card)
        .into_iter()
        .map(|action| action.label.clone())
        .collect();
    (!labels.is_empty()).then(|| Row::Buttons {
        id: id.cloned(),
        labels,
    })
}

/// What goes dim after a line: which app a message was sent from, after its header, and
/// the session an app's post is about, at the end of its first row.
fn hints(item: &Item<'_>) -> (String, String) {
    match item {
        Item::Message {
            sent_from: Some(app),
            show_sent_from: true,
            ..
        } => (format!("· {app}"), String::new()),
        Item::Landing(landing) => (
            String::new(),
            landing
                .session
                .map(|session| format!("· {session}"))
                .unwrap_or_default(),
        ),
        _ => (String::new(), String::new()),
    }
}

/// What `conversation`, in `chat`, shows, its items by day in `zone`: only what people
/// say, and whatever mentions us, when `people` is all that's wanted.
fn shown<'a>(
    chat: &Chat,
    conversation: &'a Conversation,
    zone: Zone,
    people: bool,
) -> Vec<Item<'a>> {
    let all = chat.items(conversation, |time| zone.date(time));
    if people {
        people_only(all, chat.me())
    } else {
        all
    }
}

/// What `item` came from, unless it's several things or none.
fn entry_of<'a>(item: &Item<'a>) -> Option<&'a Entry> {
    match item {
        Item::Message { entry, .. }
        | Item::Notice { entry, .. }
        | Item::Action { entry, .. }
        | Item::Event { entry, .. }
        | Item::File { entry, .. }
        | Item::Note { entry, .. } => Some(entry),
        Item::Landing(landing) => Some(landing.entry),
        Item::Day(_) | Item::Membership(_) => None,
    }
}

/// Whether `item` is the message `id`.
fn holds(item: &Item<'_>, id: &str) -> bool {
    entry_of(item).is_some_and(|entry| entry.id.as_deref() == Some(id))
}

/// Whether `item` carries on a run of what the same agent said, as zirc-chat says: its
/// posts read on under one header, as a person's do.
fn runs_on(item: &Item<'_>) -> bool {
    matches!(
        item,
        Item::Event {
            continues: true,
            ..
        }
    )
}

/// `item` as a system row, if it's one: a machine's event, its words its own when
/// zirc-chat has none for it ([`Parts::words`]), or people coming and going.
fn system_parts(item: &Item<'_>) -> Option<Parts> {
    match item {
        Item::Event { parts, text, .. } => Some(Parts {
            details: parts.words(text),
            ..parts.clone()
        }),
        Item::Membership(membership) => Some(membership.parts()),
        _ => None,
    }
}

/// Marks a system row's `rows` as mentioning us when it does, as a message's are.
fn mark_mentions(rows: &mut [Row], mentions: bool) {
    for row in rows {
        if let Row::System { mention, .. } | Row::Text { mention, .. } = row {
            *mention = mentions;
        }
    }
}

/// How a system row's text is laid out: its width, what to call an agent session it names,
/// and how the terminal shows links.
type Shape<'a> = (usize, &'a dyn Fn(&SpanKind) -> Option<String>, Links);

/// A system row, `parts`, at `time`. A short one, or any `compact`, is one line, `dana's
/// agent pushed main`, wrapped; otherwise a header, `dana's agent · intent · 14:03`, with
/// what it's about under it. Its paths follow, unless compact, on a dim row cut to fit.
fn system_rows(
    rows: &mut Vec<Row>,
    parts: &Parts,
    time: &str,
    id: Option<&str>,
    compact: bool,
    (text_width, named, link_mode): Shape<'_>,
    (agent, pill, continues): (Option<Agent>, Option<&'static str>, bool),
) {
    // Agent sessions named in it go by their names, drawn as mentions; the branch and
    // commit it names link to their pages, where the terminal can.
    let (details, mut marks) = label(&strip_formatting(&parts.details), Links::Plain, named, &[]);
    let parts = &Parts {
        details,
        ..parts.clone()
    };
    if link_mode == Links::Labelled {
        marks.extend(
            parts
                .links
                .iter()
                .map(|(word, url)| (word.clone(), Some(url.clone()))),
        );
    }
    // People coming and going are one line, as zirc-chat decides (`Parts::compact`), and
    // everything is in the IRC display mode, its icon in front. Anything else is drawn as a
    // message: its actor's header, unless it carries on their run, then its body.
    let one_line = compact || parts.compact;
    let phrase = parts.phrase();
    let agent = agent.map(|agent| Agent {
        detail: if one_line {
            String::new()
        } else {
            agent.detail
        },
        ..agent
    });
    let lines = if one_line {
        let text_width = if agent.is_some() && !compact {
            text_width.saturating_sub(AGENT_MARK_WIDTH)
        } else {
            text_width
        };
        let (said, width) = time_room(parts.line(), text_width, time, (agent.is_some(), compact));
        let wrapped = wrap(&said, width);
        let marked = fragments(&wrapped, &marks, width);
        let mut lines = wrapped.into_iter().zip(marked);
        let (first, first_links) = lines.next().unwrap_or_default();
        // Who is drawn apart, in bold, when it fits on the first line.
        let (actor, verb) = match first.strip_prefix(parts.actor.as_str()) {
            Some(rest) if !parts.actor.is_empty() => (parts.actor.clone(), rest.trim_start()),
            _ => (String::new(), first.as_str()),
        };
        rows.push(Row::System {
            time: time.to_owned(),
            icon: parts.icon,
            actor,
            verb: verb.to_owned(),
            id: id.map(str::to_owned),
            agent: agent.clone(),
            links: first_links,
            mention: false,
        });
        lines.map(|line| (line, Tone::Status)).collect::<Vec<_>>()
    } else {
        // Its header, a message's: who, whose agent or an app's tag, and when. No verb, no
        // kind word and no AGENT tag: its body's lead says what it did.
        if !continues {
            rows.push(Row::Header {
                nick: fit(&parts.actor, text_width),
                time: time.to_owned(),
                tone: Tone::Message,
                hint: String::new(),
                id: id.map(str::to_owned),
                pill,
                agent,
            });
        }
        // Its body: zirc-chat's lead, the marker and what it did, then what it was, all
        // wrapped, never cut.
        let mut lines = Vec::new();
        for (text, tone) in body(parts, phrase.as_deref()) {
            if text.is_empty() {
                continue;
            }
            let wrapped = wrap(&text, text_width);
            let marked = fragments(&wrapped, &marks, text_width);
            lines.extend(wrapped.into_iter().zip(marked).map(|line| (line, tone)));
        }
        lines
    };
    let text_row = |(text, links): (String, Vec<Link>), tone: Tone| Row::Text {
        time: String::new(),
        nick: String::new(),
        text,
        tone,
        mention: false,
        hint: String::new(),
        id: id.map(str::to_owned),
        links,
    };
    rows.extend(lines.into_iter().map(|(line, tone)| text_row(line, tone)));
    if !compact && phrase.is_none() && !parts.paths.is_empty() {
        rows.push(text_row(
            (paths_line(&parts.paths, text_width), Vec::new()),
            Tone::Path,
        ));
    }
}

/// An event's body as a message draws it: zirc-chat's lead, the marker and what it did, then
/// its details, what it was; or the marker and its `phrase` ([`Parts::phrase`]), when it's
/// said as one.
fn body(parts: &Parts, phrase: Option<&str>) -> [(String, Tone); 2] {
    match phrase {
        Some(phrase) => [
            (
                format!("{} {phrase}", parts.icon.glyph()),
                Tone::Lead(parts.icon),
            ),
            (String::new(), Tone::Message),
        ],
        None => [
            (parts.lead(), Tone::Lead(parts.icon)),
            (parts.details.clone(), Tone::Message),
        ],
    }
}

/// A one-line system row's words, `said`, and the width to wrap them in, `text_width` less
/// room for its time at the end, ` · 14:03`, which is never cut: an `agent`'s doings are
/// cut short to one line, anything else wraps; `compact`, IRC's way, the time is in front.
fn time_room(
    said: String,
    text_width: usize,
    time: &str,
    (agent, compact): (bool, bool),
) -> (String, usize) {
    if compact {
        return (said, text_width);
    }
    let width = text_width.saturating_sub(time.width() + 3);
    let said = if agent { fit(&said, width) } else { said };
    (said, width)
}

/// `paths` on one row of at most `width` columns, those that don't fit counted: `src/a.rs
/// src/b.rs +3`.
fn paths_line(paths: &[String], width: usize) -> String {
    let mut line = String::new();
    for (i, path) in paths.iter().enumerate() {
        let gap = if line.is_empty() { "" } else { "  " };
        let left = paths.len() - i - 1;
        let more = if left == 0 {
            String::new()
        } else {
            format!("  +{left}")
        };
        if line.width() + gap.width() + path.width() + more.width() > width {
            let rest = format!("+{}", paths.len() - i);
            return if line.is_empty() {
                fit(path, width.saturating_sub(rest.width() + 2)) + "  " + &rest
            } else {
                format!("{line}  {rest}")
            };
        }
        line.push_str(gap);
        line.push_str(path);
    }
    line
}

/// The quote of what `piece` answers, if it's a reply, as a row.
fn quote_row(piece: &Piece) -> Option<Row> {
    Some(Row::Text {
        time: String::new(),
        nick: String::new(),
        text: piece.quote.clone()?,
        tone: Tone::Quote,
        mention: false,
        hint: String::new(),
        id: piece.id.clone(),
        links: Vec::new(),
    })
}

/// `piece` as IRC clients show it: the time and the nick in front of its first row.
fn compact_rows(rows: &mut Vec<Row>, piece: &Piece, text_width: usize) {
    rows.extend(quote_row(piece));
    // A guest is tagged in front of what they say, as the nick column has no room.
    let text = match piece.pill {
        Some(GUEST_PILL) => format!("[{GUEST_PILL}] {}", piece.text),
        _ => piece.text.clone(),
    };
    let wrapped = wrap(&text, text_width);
    let links = fragments(&wrapped, &piece.links, text_width);
    for (i, (text, links)) in wrapped.into_iter().zip(links).enumerate() {
        let first = i == 0;
        rows.push(Row::Text {
            time: if first {
                piece.time.clone()
            } else {
                String::new()
            },
            nick: if first {
                piece.nick.clone()
            } else {
                String::new()
            },
            text,
            tone: piece.tone,
            mention: piece.mention,
            hint: match (first, piece.hint.is_empty()) {
                (false, _) => String::new(),
                (true, true) => piece.after.clone(),
                (true, false) => piece.hint.clone(),
            },
            id: piece.id.clone(),
            links,
        });
    }
}

/// `piece` as zirc's other apps show it: what someone said under a header, its nick and
/// time, unless it continues their run; anything else on its own, after its mark.
fn standard_rows(rows: &mut Vec<Row>, piece: &Piece, text_width: usize) {
    if piece.said && !piece.continues {
        rows.push(Row::Header {
            nick: piece.nick.clone(),
            time: piece.time.clone(),
            tone: piece.tone,
            hint: piece.hint.clone(),
            id: piece.id.clone(),
            pill: piece.pill,
            agent: piece.agent.clone(),
        });
    }
    rows.extend(quote_row(piece));
    let wrapped = wrap(&piece.text, text_width);
    let links = fragments(&wrapped, &piece.links, text_width);
    for (i, (text, links)) in wrapped.into_iter().zip(links).enumerate() {
        rows.push(Row::Text {
            time: String::new(),
            // What nobody said has its mark in front, and an app's post its outcome's icon.
            nick: match piece.tone {
                _ if i > 0 => String::new(),
                _ if !piece.said => piece.nick.clone(),
                Tone::App(Some(icon)) => icon.glyph().to_owned(),
                _ => String::new(),
            },
            text,
            tone: piece.tone,
            mention: piece.mention,
            // The session an app's post is about, dim at the end of its first row.
            hint: if i == 0 {
                piece.after.clone()
            } else {
                String::new()
            },
            id: piece.id.clone(),
            links,
        });
    }
}

/// The reactions to the message `id`, in rows of at most `width` columns.
fn reaction_rows(conversation: &Conversation, id: &str, me: &str, width: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut chips: Vec<Chip> = Vec::new();
    let mut used = 0;
    for chip in conversation.chips(id, me) {
        let wide = chip_text(&chip).width();
        if !chips.is_empty() && used + CHIP_GAP + wide > width {
            rows.push(Row::Reactions {
                id: id.to_owned(),
                chips: std::mem::take(&mut chips),
            });
        }
        used = if chips.is_empty() {
            wide
        } else {
            used + CHIP_GAP + wide
        };
        chips.push(chip);
    }
    if !chips.is_empty() {
        rows.push(Row::Reactions {
            id: id.to_owned(),
            chips,
        });
    }
    rows
}

/// The ids of the messages that can be reacted to in `rows`, oldest first, each once.
#[must_use]
pub fn message_ids(rows: &[Row]) -> Vec<&str> {
    let mut ids: Vec<&str> = Vec::new();
    for row in rows {
        if let Row::Text { id: Some(id), .. } | Row::System { id: Some(id), .. } = row
            && ids.last() != Some(&id.as_str())
        {
            ids.push(id);
        }
    }
    ids
}

/// The width of the nick column: as wide as the widest nick that talks, or app that posts,
/// within limits.
#[must_use]
pub fn nick_width(conversation: &Conversation) -> usize {
    conversation
        .entries
        .iter()
        .filter_map(|entry| {
            entry.speaker().or_else(|| {
                (matches!(entry.kind, EntryKind::Zirc(_))
                    && sender_kind(&entry.from, entry.owner.as_deref()) == SenderKind::Integration)
                    .then_some(entry.from.as_str())
            })
        })
        .map(|nick| nick.width() + 2)
        .max()
        .unwrap_or(0)
        .clamp(NICK_WIDTH.0, NICK_WIDTH.1)
}

/// What a line shows in the nick column and as text, how it reads, and whether it carries
/// on a run. System rows are laid out by [`system_rows`] instead. A notice is headed by its
/// sender's name, as a message's head names them ([`Chat::speaker`]): zirc's own as zirc.
fn columns(chat: &Chat, item: &Item<'_>, compact: bool) -> (String, String, Tone, bool) {
    match item {
        Item::Message {
            entry,
            text,
            continues,
            ..
        } => (
            entry.from.clone(),
            (*text).to_owned(),
            if sender_kind(&entry.from, entry.owner.as_deref()) == SenderKind::Integration {
                Tone::App(None)
            } else {
                Tone::Message
            },
            *continues,
        ),
        Item::Notice {
            entry,
            text,
            continues,
        } => (
            format!("-{}-", chat.speaker(entry).name),
            (*text).to_owned(),
            Tone::Notice,
            *continues,
        ),
        Item::Action { entry, text, .. } => (
            "*".into(),
            format!("{} {text}", entry.from),
            Tone::Action,
            false,
        ),
        // An app's post, under its name like anyone's, its outcome's icon in front: in the
        // indent, or compact, before the text.
        Item::Landing(landing) => {
            let icon = Some(landing.parts().icon).filter(|icon| *icon != Icon::Post);
            let (text, tone) = match icon {
                Some(icon) if compact => (
                    format!("{} {}", icon.glyph(), landing.plain()),
                    Tone::App(None),
                ),
                _ => (landing.plain(), Tone::App(icon)),
            };
            (landing.entry.from.clone(), text, tone, landing.continues)
        }
        // The link opens it: terminals make URLs clickable, and zirc.dev asks the person to
        // sign in.
        Item::File {
            file,
            from,
            continues,
            ..
        } => (
            from.clone(),
            format!(
                "{} ({}) {}",
                strip_formatting(&file.name),
                size_text(file.size),
                file.url
            ),
            Tone::Message,
            *continues,
        ),
        Item::Note { entry, text } => {
            let (mark, tone) = if matches!(entry.kind, EntryKind::Info(_)) {
                ("!", Tone::Info)
            } else {
                ("·", Tone::Status)
            };
            (mark.into(), text.clone(), tone, false)
        }
        Item::Day(_) | Item::Event { .. } | Item::Membership(_) => {
            (String::new(), String::new(), Tone::Status, false)
        }
    }
}

/// `name` cut to `width` columns, with `…` when cut: how every name too long for its place
/// shows.
#[must_use]
pub fn fit(name: &str, width: usize) -> String {
    if name.width() <= width {
        return name.to_owned();
    }
    let mut out = String::new();
    for c in name.chars() {
        if out.width() + c.width().unwrap_or(0) + 1 > width {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

/// `text` in rows of at most `width` columns, broken at spaces where it can and at its own
/// line breaks.
#[must_use]
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for line in text.split('\n') {
        let mut row = String::new();
        let mut row_width = 0;
        for word in line.split(' ') {
            let word_width = word.width();
            let gap = usize::from(!row.is_empty());
            if row_width + gap + word_width <= width {
                if gap == 1 {
                    row.push(' ');
                }
                row.push_str(word);
                row_width += gap + word_width;
                continue;
            }
            if !row.is_empty() {
                rows.push(std::mem::take(&mut row));
                row_width = 0;
            }
            // A word wider than a row is cut wherever it has to be.
            for c in word.chars() {
                let w = c.width().unwrap_or(0);
                if row_width + w > width {
                    rows.push(std::mem::take(&mut row));
                    row_width = 0;
                }
                row.push(c);
                row_width += w;
            }
        }
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unpressed(_: &str, _: &Card) -> Option<String> {
        None
    }

    /// Us as `me`, in a chat that knows nothing more: no guests, everyone by their nick and
    /// sessions by their ids.
    fn who() -> Who<'static> {
        let mut chat = Chat::new(
            "me",
            "irc.zirc.test",
            Vec::new(),
            zirc_core::chat::Options::default(),
        );
        // The machines these tests hear from, whose their hosts say they are.
        for machine in [":mattias-mbp!u@machine/mattias", ":dana-mbp!u@machine/dana"] {
            let _ = chat.receive(format!("{machine} JOIN #api").parse().unwrap());
        }
        let chat: &'static Chat = Box::leak(Box::new(chat));
        Who {
            chat,
            pressed: &unpressed,
        }
    }
    use chrono::TimeZone as _;
    use zirc_core::chat::Kind;

    fn utc() -> Zone {
        Zone::Fixed(FixedOffset::east_opt(0).unwrap())
    }

    /// What `from` said at `hour`:`minute` on the `day`th. These tests' machines,
    /// `mattias-mbp` and `dana-mbp`, are their owners', as their hosts said.
    fn at(day: u32, hour: u32, minute: u32, (from, kind): (&str, EntryKind)) -> Entry {
        Entry {
            from: from.into(),
            owner: from.strip_suffix("-mbp").map(str::to_owned),
            kind,
            ..Entry::info(
                Utc.with_ymd_and_hms(2026, 10, day, hour, minute, 0)
                    .unwrap(),
                String::new(),
            )
        }
    }

    fn says<'a>(nick: &'a str, text: &str) -> (&'a str, EntryKind) {
        (nick, EntryKind::Message(text.into()))
    }

    fn shown(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Divider(day) => format!("--- {day}"),
                Row::New => "--- New".to_owned(),
                Row::Buttons { labels, .. } => format!("{:13} {}", "", labels.join(" · ")),
                Row::System {
                    time,
                    icon,
                    actor,
                    verb,
                    ..
                } => format!("{time:5} {:>7} {actor}|{verb}", icon.glyph()),
                Row::Header {
                    nick,
                    time,
                    hint,
                    pill,
                    ..
                } => {
                    let pill = pill.map(|pill| format!(" {pill}")).unwrap_or_default();
                    format!("{nick}{pill} {time}  {hint}").trim_end().to_owned()
                }
                Row::Text {
                    time,
                    nick,
                    text,
                    hint,
                    ..
                } => format!("{time:5} {nick:>7} {text}  {hint}")
                    .trim_end()
                    .to_owned(),
                Row::Reactions { chips, .. } => {
                    let chips: Vec<String> = chips
                        .iter()
                        .map(|chip| {
                            let mine = if chip.mine { "*" } else { "" };
                            format!("[{}{mine}]", chip_text(chip).trim())
                        })
                        .collect();
                    format!("{:13} {}", "", chips.join(" "))
                }
            })
            .collect()
    }

    #[test]
    fn shows_reactions_under_what_they_react_to() {
        use zirc_core::chat::{Chat, Link, Options};
        let mut chat = Chat::new("me", "irc.zirc.test", Vec::new(), Options::default());
        let _ = chat.connected(Link {
            nick: "me".into(),
            metadata: false,
            chathistory: false,
            echo: true,
            read_marker: false,
            trim: true,
            multiline: None,
        });
        for line in [
            ":me!u@h JOIN #api",
            "@msgid=m1;time=2026-10-30T09:00:00.000Z :alice!u@h PRIVMSG #api :shipped it",
            "@msgid=m2;time=2026-10-30T09:01:00.000Z :bob!u@h PRIVMSG #api :nice",
            "@+draft/react=🎉;+draft/reply=m1 :bob!u@h TAGMSG #api",
            "@+draft/react=🎉;+draft/reply=m1 :me!u@h TAGMSG #api",
            "@+draft/react=🚀;+draft/reply=m1 :bob!u@h TAGMSG #api",
        ] {
            let _ = chat.receive(line.parse().unwrap());
        }
        let api = chat.conversation("#api").unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 10, 30).unwrap();
        let rows = layout(
            api,
            who(),
            None,
            80,
            (utc(), today),
            (true, Links::Plain, false),
        );
        assert_eq!(
            shown(&rows),
            [
                "--- Today",
                "09:00   alice shipped it",
                "              [🎉 2*] [🚀 1]",
                "09:01     bob nice",
            ]
        );
        assert_eq!(message_ids(&rows), ["m1", "m2"]);
        // Too many for a row go on to the next.
        let narrow = layout(
            api,
            who(),
            None,
            26,
            (utc(), today),
            (true, Links::Plain, false),
        );
        assert_eq!(
            shown(&narrow)[2..4],
            ["              [🎉 2*]", "              [🚀 1]"]
        );
        // Where each is, for clicks.
        let Row::Reactions { chips, .. } = &rows[2] else {
            panic!("{rows:?}")
        };
        assert_eq!(chip_at(chips, 0).map(|c| c.emoji.as_str()), Some("🎉"));
        assert_eq!(chip_at(chips, 6), None, "the gap");
        assert_eq!(chip_at(chips, 7).map(|c| c.emoji.as_str()), Some("🚀"));
    }

    #[test]
    fn hints_which_app_a_run_was_sent_from() {
        let mut api = Conversation::new("#api", Kind::Channel);
        for (minute, text, app) in [
            (0, "hi", Some("desktop")),
            (1, "still here", Some("desktop")),
            (2, "on my phone", Some("android")),
            (3, "from irssi", None),
        ] {
            api.entries.push(Entry {
                sent_from: app.map(str::to_owned),
                ..at(30, 9, minute, says("mattias", text))
            });
        }
        let today = NaiveDate::from_ymd_opt(2026, 10, 30).unwrap();
        assert_eq!(
            shown(&layout(
                &api,
                who(),
                None,
                80,
                (utc(), today),
                (true, Links::Plain, false)
            )),
            [
                "--- Today",
                "09:00 mattias hi  · desktop",
                "09:01         still here",
                "09:02         on my phone  · android",
                "09:03         from irssi",
            ]
        );
    }

    #[test]
    fn groups_runs_and_divides_days() {
        let mut api = Conversation::new("#api", Kind::Channel);
        for entry in [
            at(29, 23, 50, says("alice", "late")),
            at(30, 9, 0, says("alice", "morning")),
            at(30, 9, 0, says("alice", "again")),
            at(30, 9, 3, says("alice", "and again")),
            at(30, 9, 4, says("bob", "hi")),
            at(30, 9, 30, says("bob", "much later")),
            at(30, 9, 31, ("carol", EntryKind::Join)),
        ] {
            api.entries.push(entry);
        }
        let today = NaiveDate::from_ymd_opt(2026, 10, 30).unwrap();
        assert_eq!(
            shown(&layout(
                &api,
                who(),
                None,
                80,
                (utc(), today),
                (true, Links::Plain, false)
            )),
            [
                "--- Yesterday",
                "23:50   alice late",
                "--- Today",
                "09:00   alice morning",
                "              again",
                "09:03         and again",
                "09:04     bob hi",
                "09:30     bob much later",
                "09:31       + |carol joined",
            ]
        );
        let older = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        assert_eq!(
            shown(&layout(
                &api,
                who(),
                None,
                80,
                (utc(), older),
                (true, Links::Plain, false)
            ))[0],
            "--- Thursday 29 October 2026"
        );
    }

    #[test]
    fn shows_zirc_events_as_one_line_and_wraps_long_text() {
        let mut api = Conversation::new("#api", Kind::Channel);
        let commit = zirc_proto::Message::new(
            "cc-12",
            zirc_proto::Event::Commit {
                paths: Vec::new(),
                branch: "main".into(),
                sha: "abc".into(),
                subject: "Fix".into(),
                repo: None,
            },
        );
        api.entries
            .push(at(30, 9, 0, ("mattias-mbp", EntryKind::Zirc(commit))));
        api.entries
            .push(at(30, 9, 1, says("alice", "one two three four five")));
        let today = NaiveDate::from_ymd_opt(2026, 10, 30).unwrap();
        // 5 + 1 + 7 (alice + 2) + 1 leaves 18 columns of text.
        assert_eq!(
            shown(&layout(
                &api,
                who(),
                None,
                32,
                (utc(), today),
                (true, Links::Plain, false)
            )),
            [
                "--- Today",
                // An agent session goes by its name, or else its id.
                "09:00       ● cc-12|committed:",
                "              Fix  (main abc)",
                "09:01   alice one two three four",
                "              five",
            ]
        );
    }

    fn commit(session: &str, sha: &str, tool: Option<&str>) -> EntryKind {
        let mut message = zirc_proto::Message::new(
            session,
            zirc_proto::Event::Commit {
                paths: Vec::new(),
                branch: "main".into(),
                sha: sha.into(),
                subject: "Fix".into(),
                repo: None,
            },
        );
        message.tool = tool.map(str::to_owned);
        EntryKind::Zirc(message)
    }

    #[test]
    fn says_a_commit_and_its_push_in_one_row() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 30).unwrap();
        let mut api = Conversation::new("#api", Kind::Channel);
        let push = zirc_proto::Message::new(
            "cc-12",
            zirc_proto::Event::Push {
                branch: "main".into(),
                head: "abc1234".into(),
                forced: false,
                subjects: Vec::new(),
                paths: Vec::new(),
                base: None,
                lines: std::collections::BTreeMap::new(),
                repo: None,
            },
        );
        api.entries
            .push(at(30, 9, 0, ("mattias-mbp", commit("cc-12", "abc", None))));
        api.entries
            .push(at(30, 9, 1, ("mattias-mbp", EntryKind::Zirc(push))));
        let rows = layout(
            &api,
            who(),
            None,
            80,
            (utc(), today),
            (false, Links::Plain, false),
        );
        // One message: its header, whose agent too, then what it was under it.
        assert_eq!(
            shown(&rows),
            [
                "--- Today",
                "cc-12 09:00",
                "              ● committed and pushed to main",
                "              Fix  (main abc)"
            ]
        );
        assert!(
            matches!(&rows[1], Row::Header { agent: Some(agent), .. } if !agent.detail.is_empty())
        );
    }

    fn zirc(nick: &str, event: zirc_proto::Event) -> (&str, EntryKind) {
        (
            nick,
            EntryKind::Zirc(zirc_proto::Message::new("cc-12", event)),
        )
    }

    fn landing(text: &str) -> zirc_proto::Event {
        zirc_proto::Event::custom(zirc_proto::ci::LANDING_KIND, text)
    }

    #[test]
    fn draws_a_new_divider_above_the_first_unread_without_breaking_the_run() {
        let mut api = Conversation::new("#api", Kind::Channel);
        for (minute, id) in [(0, "a1"), (1, "a2")] {
            api.entries.push(Entry {
                id: Some(id.into()),
                ..at(30, 9, minute, says("alice", id))
            });
        }
        let today = NaiveDate::from_ymd_opt(2026, 10, 30).unwrap();
        assert_eq!(
            shown(&layout(
                &api,
                who(),
                Some("a2"),
                80,
                (utc(), today),
                (true, Links::Plain, false)
            )),
            [
                "--- Today",
                "09:00   alice a1",
                "--- New",
                // Still alice's run: no second header for her.
                "09:01         a2",
            ]
        );
    }

    #[test]
    fn shows_an_apps_posts_as_its_run_with_the_outcome_in_front_and_the_session_after() {
        let mut api = Conversation::new("#zirc", Kind::Channel);
        for (minute, text) in [
            (0, "cc-cdbba7: landed web-look as 3d5fe8a (build 435)"),
            (
                1,
                "cc-cdbba7: build 436 failed in test: //crates/zirc-tui:zirc-tui_test",
            ),
        ] {
            api.entries.push(at(
                30,
                9,
                minute,
                (
                    "zirc-ci",
                    EntryKind::Zirc(zirc_proto::Message::new("ci", landing(text))),
                ),
            ));
        }
        let today = NaiveDate::from_ymd_opt(2026, 10, 30).unwrap();
        let rows = layout(
            &api,
            who(),
            None,
            80,
            (utc(), today),
            (false, Links::Plain, false),
        );
        assert_eq!(
            shown(&rows),
            [
                "--- Today",
                // An app by its glyph, as zirc's other apps show its avatar.
                "CI APP 09:00",
                "            ✓ landed web-look as 3d5fe8a (build 435)  · cc-cdbba7",
                "            ✗ build 436 failed in test: //crates/zirc-tui:zirc-tui_test  · cc-cdbba7",
            ]
        );
        assert!(matches!(
            rows[2],
            Row::Text {
                tone: Tone::App(Some(Icon::Landed)),
                ..
            }
        ));
        // Compact, the icon goes before the text, and the app's name only once.
        assert_eq!(
            shown(&layout(
                &api,
                who(),
                None,
                80,
                (utc(), today),
                (true, Links::Plain, false)
            ))[1..],
            [
                "09:00 zirc-ci ✓ landed web-look as 3d5fe8a (build 435)  · cc-cdbba7",
                "09:01         ✗ build 436 failed in test: //crates/zirc-tui:zirc-tui_test  · cc-cdbba7",
            ]
        );
    }

    #[test]
    fn heads_an_event_as_a_message_and_starts_its_body_with_the_lead() {
        let mut api = Conversation::new("#api", Kind::Channel);
        let intent = zirc_proto::Event::Intent {
            summary: "split the login form".into(),
            paths: (1..=6)
                .map(|n| format!("web/src/login/part{n}.tsx"))
                .collect(),
            expires: None,
            lasts: None,
        };
        api.entries.push(at(30, 14, 3, zirc("dana-mbp", intent)));
        api.entries.push(at(30, 14, 4, ("bob", EntryKind::Join)));
        let today = NaiveDate::from_ymd_opt(2026, 10, 30).unwrap();
        let rows = layout(
            &api,
            who(),
            None,
            60,
            (utc(), today),
            (false, Links::Plain, false),
        );
        assert_eq!(
            shown(&rows),
            [
                "--- Today",
                // An agent's activity as a message: its header, who and when with no verb
                // or kind word; then zirc-chat's lead, what it's about, and its paths.
                "cc-12 14:03",
                "              ✎ is about to change",
                "              split the login form",
                "              web/src/login/part1.tsx  web/src/login/part2.tsx  +4",
                "14:04       + |bob joined",
            ]
        );
        assert!(matches!(
            rows[2],
            Row::Text {
                tone: Tone::Lead(Icon::Intent),
                ..
            }
        ));
        assert!(matches!(
            rows[4],
            Row::Text {
                tone: Tone::Path,
                ..
            }
        ));
    }

    #[test]
    fn lines_up_a_tables_columns() {
        assert_eq!(
            lined_up(
                "builds:\n| target | took |\n| :-: | --: |\n| `//web:e2e` | 90s |\n| 漢字 |\n\ndone"
            ),
            [
                "builds:",
                "  target    │ took",
                "────────────┼─────",
                "`//web:e2e` │  90s",
                "   漢字     │",
                "",
                "done",
            ]
            .join("\n")
        );
        assert_eq!(lined_up("a | b"), "a | b");
    }

    #[test]
    fn wraps_at_spaces_line_breaks_and_inside_long_words() {
        assert_eq!(wrap("aaa bbb ccc", 7), ["aaa bbb", "ccc"]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("one\ntwo", 10), ["one", "two"]);
        assert_eq!(wrap("", 10), [""]);
        assert_eq!(wrap("日本語です", 4), ["日本", "語で", "す"]);
    }

    #[test]
    fn heads_each_run_with_who_and_when_as_the_other_apps_do() {
        let mut api = Conversation::new("#api", Kind::Channel);
        for entry in [
            at(30, 9, 0, says("alice", "morning")),
            at(
                30,
                9,
                1,
                says(
                    "alice",
                    "can you look at the login bug? it's been failing since",
                ),
            ),
            at(30, 9, 2, ("bob", EntryKind::Join)),
            at(30, 9, 3, says("bob", "on it")),
        ] {
            api.entries.push(entry);
        }
        let today = NaiveDate::from_ymd_opt(2026, 10, 30).unwrap();
        let rows = layout(
            &api,
            who(),
            None,
            40,
            (utc(), today),
            (false, Links::Plain, false),
        );
        let shown: Vec<String> = rows
            .iter()
            .map(|row| match row {
                Row::Divider(day) => format!("--- {day}"),
                Row::New => "--- New".to_owned(),
                Row::Buttons { labels, .. } => labels.join(" · "),
                Row::System {
                    icon, actor, verb, ..
                } => format!("{} {actor}|{verb}", icon.glyph()),
                Row::Header { nick, time, .. } => format!("{nick} {time}"),
                Row::Text { nick, text, .. } => format!("{nick:2}{text}"),
                Row::Reactions { .. } => String::new(),
            })
            .collect();
        assert_eq!(
            shown,
            [
                "--- Today",
                "alice 09:00",
                "  morning",
                "  can you look at the login bug? it's",
                "  been failing since",
                "+ |bob joined",
                "bob 09:03",
                "  on it",
            ]
        );
    }

    #[test]
    fn cuts_long_nicks() {
        assert_eq!(fit("alexandria", 6), "alexa…");
        assert_eq!(fit("bob", 6), "bob");
    }
}
