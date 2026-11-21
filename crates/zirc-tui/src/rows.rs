//! A conversation laid out as rows of the scrollback, from the lines zirc-chat gives every
//! app ([`Chat::lines`]): the core decides each line's shape, its head, whether it carries
//! on a run, its words as blocks, its quote, its reactions and its card; this lays them out
//! in rows of the terminal's width, long text wrapped. The UI only has to colour them in.

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, Offset as _, Utc};
use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};
use zirc_core::chat::lines::Line;
use zirc_core::chat::{
    APP_TAG, CellAlign, Chat, EDITED, Face, Icon, LineFile, LineKind, LineReaction, LineSpeaker,
    RichBlock, RichSpan, RichSpanKind, RowShape, SHOWN_PATHS, SystemBody, SystemLine, link_label,
    show_more_paths_label,
};

use crate::hyperlink::{Link, Links, fragments};

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

    /// How far its clock is from UTC now, which zirc-chat's lines are told.
    #[must_use]
    pub fn offset(self) -> FixedOffset {
        match self {
            Self::Local => chrono::Local::now().offset().fix(),
            Self::Fixed(offset) => offset,
        }
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
        /// What came of our last press of one, until the card changes.
        said: Option<String>,
    },
    /// A quiet line, as zirc-chat shapes it ([`RowShape::Quiet`], [`RowShape::Brief`],
    /// [`RowShape::Pointer`]): its marker, what it says, muted, then when, dim, when it
    /// shows a time. What it says further, when it wraps, follows in [`Tone::Status`] rows.
    System {
        /// When, `12:04`; empty for a line that shows no time.
        time: String,
        /// Its marker, zirc-chat's [`SystemLine::glyph`], or nothing.
        glyph: String,
        /// What it says on this row.
        text: String,
        /// The id of the message, when it can be reacted to.
        id: Option<String>,
        /// When an agent session did it, its mark.
        agent: Option<Agent>,
        /// The links on its row.
        links: Vec<Link>,
        /// Whether it mentions us: zirc-chat's `Line::mention`.
        mention: bool,
    },
    /// Part of a line's body.
    Text {
        /// What goes in the indent in front of the text: a marker such as `*`, a card's
        /// outcome, or nothing.
        nick: String,
        /// The text on this row.
        text: String,
        /// How it reads.
        tone: Tone,
        /// Whether the line mentions us.
        mention: bool,
        /// What goes dim at the end of the row: the session a card is about, on its first.
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
        /// What goes dim after the time: in an agent session's conversation, the room it
        /// was said in ([`Line::room`]).
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

/// One reaction to a message, as zirc-chat counts it ([`Line::reactions`]).
pub use zirc_core::chat::LineReaction as Chip;

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

/// An agent session heading a row, as zirc-chat's [`LineSpeaker`] says, where other apps
/// draw its tool on its colour: a mark for its tool in its own colour, and whose agent it is
/// after its name, `mattias's agent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    /// One cell for its tool: see [`zirc_core::chat::tool_mark`].
    pub mark: &'static str,
    /// Its colour's hue in degrees, below 360, the same every time for the same session.
    pub hue: u16,
    /// What follows its name, muted: `mattias's agent`; empty for the machine itself.
    pub detail: String,
}

/// The columns an agent's mark takes in front of a quiet row's marker, with its space.
pub const AGENT_MARK_WIDTH: usize = 2;

/// An agent's face as its mark, with what follows its name; `None` for anyone else.
fn agent(speaker: &LineSpeaker) -> Option<Agent> {
    match &speaker.face {
        Face::Agent { tool, hue, .. } => Some(Agent {
            mark: zirc_core::chat::tool_mark(tool.as_deref()),
            hue: *hue,
            detail: speaker.detail.clone().unwrap_or_default(),
        }),
        _ => None,
    }
}

/// What's marked after the name heading `line`, as zirc-chat says ([`Chat::name_pill`]): a
/// guest as outside, an app by its tag.
fn pill(chat: &Chat, line: &Line, speaker: &LineSpeaker) -> Option<&'static str> {
    if line.from.is_empty() {
        speaker.app.then_some(APP_TAG)
    } else {
        chat.name_pill(&line.from, speaker.app)
    }
}

/// How a row reads, for colouring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tone {
    /// A message.
    Message,
    /// An action.
    Action,
    /// A notice.
    Notice,
    /// A card's words, its outcome's icon in front on its first row.
    App(Option<Icon>),
    /// An event's lead, zirc-chat's [`SystemLine::lead`]: its marker in the theme role
    /// `marker` names ([`SystemLine::marker_tone`]), what was done muted; and, when its
    /// details follow on the same row, from the byte `details.0` on, those in the role
    /// `details.1` names ([`SystemLine::details_tone`]).
    Lead {
        /// The role of its marker.
        marker: String,
        /// Where its details start on the row, and their role.
        details: Option<(usize, String)>,
    },
    /// An event's details, in the theme role zirc-chat names for them
    /// ([`SystemLine::details_tone`]).
    Details(String),
    /// Quiet words: a wrapped quiet line, a card's updates, a rule.
    Status,
    /// The paths an event is about.
    Path,
    /// What's left out, dim: a long message's "Show 34 more lines", the paths not listed.
    More,
    /// The message a reply answers, quoted above it, or a quote in a message.
    Quote,
    /// A heading, or a table's header row: bold.
    Heading,
    /// A line of a code block.
    Code,
}

/// What came of our pressing one of a card's buttons, as [`Look`] is told it.
pub trait Pressed {
    /// What came of our last press of a button on the card whose version is `key`
    /// ([`zirc_core::chat::CardButtons::key`]), if anything: it shows until the card changes.
    fn pressed(&self, key: &str) -> Option<String>;
}

impl<F: Fn(&str) -> Option<String>> Pressed for F {
    fn pressed(&self, key: &str) -> Option<String> {
        self(key)
    }
}

/// Everything lines are laid out by, but the lines.
#[derive(Clone, Copy)]
pub struct Look<'a> {
    /// The chat the conversation is in, which says who's who: guests and apps.
    chat: &'a Chat,
    /// What came of our presses.
    pressed: &'a dyn Pressed,
    /// The message the "New" divider goes above.
    divider: Option<&'a str>,
    /// How wide the text column is.
    pub text_width: usize,
    /// How the terminal shows links.
    links: Links,
}

impl<'a> Look<'a> {
    /// Lines in `chat` laid out `width` columns wide, the "New" divider above the message
    /// `divider`, what came of our presses as `pressed` says, links as the terminal shows
    /// them.
    #[must_use]
    pub fn new(
        chat: &'a Chat,
        pressed: &'a dyn Pressed,
        divider: Option<&'a str>,
        width: usize,
        links: Links,
    ) -> Self {
        Self {
            chat,
            pressed,
            divider,
            text_width: width.saturating_sub(INDENT).max(MIN_TEXT_WIDTH),
            links,
        }
    }
}

/// The rows of `lines`, as `look` lays them out.
#[must_use]
pub fn layout(look: &Look<'_>, lines: &[Line]) -> Vec<Row> {
    lines
        .iter()
        .flat_map(|line| line_rows(look, line))
        .collect()
}

/// A number that changes whenever anything [`line_rows`] lays `line` out by does, besides
/// what the stamp of the scrollback's cache covers (the width, how links show): the line,
/// whether the "New" divider is above it and what came of pressing its card.
#[must_use]
pub fn fingerprint(look: &Look<'_>, line: &Line) -> u64 {
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
    let new = is_new(look, line);
    let pressed = pressed(look, line);
    let _ = write!(hashing, "{line:?}{new}{pressed:?}");
    hashing.0.finish()
}

/// The id of the message `line` is, as its rows carry it, when it can be reacted to:
/// never a quiet line's, people coming and going or a note.
#[must_use]
pub fn line_id(line: &Line) -> Option<&str> {
    line.id.as_deref().filter(|_| line.actions.react)
}

/// Whether the "New" divider goes above `line`.
fn is_new(look: &Look<'_>, line: &Line) -> bool {
    look.divider.is_some() && look.divider == line.id.as_deref()
}

/// What came of our last press of `line`'s card, if it has one.
fn pressed(look: &Look<'_>, line: &Line) -> Option<String> {
    let buttons = line.system.as_ref()?.buttons.as_ref()?;
    look.pressed.pressed(&buttons.key)
}

/// The rows of `line`, laid out as `look` says, in the shape zirc-chat gives it
/// ([`Line::shape`]).
#[must_use]
pub fn line_rows(look: &Look<'_>, line: &Line) -> Vec<Row> {
    let mut rows = Vec::new();
    if let Some(day) = &line.day {
        rows.push(Row::Divider(day.clone()));
    }
    if is_new(look, line) {
        rows.push(Row::New);
    }
    let start = rows.len();
    match (line.shape, &line.speaker) {
        (RowShape::Message, Some(_)) if line.kind == LineKind::Action => {
            action_rows(look, line, &mut rows);
        }
        (RowShape::Message, Some(speaker)) => message_rows(look, line, speaker, &mut rows),
        (RowShape::System, Some(speaker)) => system_rows(look, line, speaker, &mut rows),
        (RowShape::Brief | RowShape::Pointer, _) => brief_rows(look, line, &mut rows),
        _ => quiet_rows(look, line, &mut rows),
    }
    let mention = line.mention;
    for row in &mut rows[start..] {
        match row {
            Row::Text { tone, .. } if *tone == Tone::Quote => {}
            Row::System {
                mention: marked, ..
            }
            | Row::Text {
                mention: marked, ..
            } => {
                *marked = mention;
            }
            _ => {}
        }
    }
    rows.extend(reaction_rows(line, look.text_width));
    rows
}

/// A row of a line's body, before it's a [`Row::Text`]: what it says, how it reads and the
/// links it shows.
struct Run {
    text: String,
    tone: Tone,
    links: Vec<Link>,
}

impl Run {
    /// It as a row of `line`'s, `nick` in the indent in front of it.
    fn row(self, line: &Line, nick: String) -> Row {
        Row::Text {
            nick,
            text: self.text,
            tone: self.tone,
            mention: false,
            hint: String::new(),
            id: line_id(line).map(str::to_owned),
            links: self.links,
        }
    }
}

/// `line`'s head: who, whose agent or their pill, and when.
fn header(look: &Look<'_>, line: &Line, speaker: &LineSpeaker) -> Row {
    Row::Header {
        nick: speaker.name.clone(),
        time: line.clock.clone(),
        tone: if line.kind == LineKind::Notice {
            Tone::Notice
        } else {
            Tone::Message
        },
        hint: line.room.clone().unwrap_or_default(),
        id: line_id(line).map(str::to_owned),
        pill: pill(look.chat, line, speaker),
        agent: agent(speaker),
    }
}

/// What someone said, or a file they shared, under their head unless it carries on their
/// run: the quote of what it answers, its words, then what's cut from a long one.
fn message_rows(look: &Look<'_>, line: &Line, speaker: &LineSpeaker, rows: &mut Vec<Row>) {
    if !line.continues {
        rows.push(header(look, line, speaker));
    }
    let width = look.text_width;
    if let Some(quote) = &line.quote {
        rows.push(
            Run {
                text: fit(&quote.line(), width),
                tone: Tone::Quote,
                links: Vec::new(),
            }
            .row(line, String::new()),
        );
    }
    let tone = if line.kind == LineKind::Notice {
        Tone::Notice
    } else {
        Tone::Message
    };
    let mut runs = match &line.file {
        Some(file) => file_runs(file, width),
        None => words(look, line, width, &tone),
    };
    edited(line, &mut runs, width);
    rows.extend(runs.into_iter().map(|run| run.row(line, String::new())));
    rows.extend(more_row(line));
}

/// An action, `/me waves`: one line of its own naming who, `* alice waves`.
fn action_rows(look: &Look<'_>, line: &Line, rows: &mut Vec<Row>) {
    let width = look.text_width;
    let mut runs = wrapped(
        (format!("{} {}", line.from, line.text), Vec::new()),
        width,
        &Tone::Action,
    );
    edited(line, &mut runs, width);
    for (at, run) in runs.into_iter().enumerate() {
        let nick = if at == 0 { "*" } else { "" };
        rows.push(run.row(line, nick.to_owned()));
    }
}

/// An event, a landing or a card, drawn as a message is: its actor's head unless it carries
/// on their run, then what zirc-chat says its body is ([`SystemLine::body`]).
fn system_rows(look: &Look<'_>, line: &Line, speaker: &LineSpeaker, rows: &mut Vec<Row>) {
    let Some(system) = &line.system else {
        return quiet_rows(look, line, rows);
    };
    if !line.continues {
        rows.push(header(look, line, speaker));
    }
    let width = look.text_width;
    if line.kind == LineKind::Landing {
        return card_rows(look, line, system, rows);
    }
    let runs = match system.body {
        SystemBody::Parts => parts_runs(look, line, system, width),
        SystemBody::Card | SystemBody::Blocks => blocks(look, &line.rich, width, &Tone::Message),
    };
    rows.extend(runs.into_iter().map(|run| run.row(line, String::new())));
    rows.extend(more_row(line));
}

/// A landing's card, or any integration's: its words with its outcome's icon in front and the
/// session it's about dim after them, its updates, its picture, what's cut from it, then its
/// buttons we may press with what came of our last press.
fn card_rows(look: &Look<'_>, line: &Line, system: &SystemLine, rows: &mut Vec<Row>) {
    let width = look.text_width;
    let mut runs = if line.rich.is_empty() {
        parts_runs(look, line, system, width)
    } else {
        blocks(look, &line.rich, width, &Tone::App(system.outcome))
    };
    if let Some(file) = &line.file {
        runs.extend(file_runs(file, width));
    }
    for (at, run) in runs.into_iter().enumerate() {
        let first = at == 0;
        let nick = match system.outcome {
            Some(icon) if first => icon.glyph().to_owned(),
            _ => String::new(),
        };
        let mut row = run.row(line, nick);
        if let Row::Text { hint, tone, .. } = &mut row
            && first
        {
            if let Some(session) = &system.session {
                hint.clone_from(&session.aside);
            }
            if !matches!(tone, Tone::App(_)) {
                *tone = Tone::App(system.outcome);
            }
        }
        rows.push(row);
    }
    rows.extend(more_row(line));
    if let Some(buttons) = &system.buttons {
        rows.push(Row::Buttons {
            id: line_id(line).map(str::to_owned),
            labels: buttons
                .actions
                .iter()
                .map(|action| action.label.clone())
                .collect(),
            said: look.pressed.pressed(&buttons.key),
        });
    }
}

/// An event's parts, as zirc-chat says them: its lead ([`SystemLine::lead`]), its details
/// on the lead's row unless they start one of their own ([`SystemLine::details_apart`]),
/// and its paths when they're listed, the first [`SHOWN_PATHS`] one a row and how many more.
fn parts_runs(look: &Look<'_>, line: &Line, system: &SystemLine, width: usize) -> Vec<Run> {
    let links = look.links;
    let lead = if system.lead.is_empty() || system.verb_links.is_empty() {
        (system.lead.clone(), Vec::new())
    } else {
        let (verb, marks) = spans_text(&system.verb_links, links);
        (format!("{} {verb}", system.glyph), marks)
    };
    let words = system.words(&line.text);
    let details = if words == system.details && !system.details_links.is_empty() {
        spans_text(&system.details_links, links)
    } else {
        (words.to_owned(), Vec::new())
    };
    let details_tone = Tone::Details(system.details_tone.clone());
    let lead_tone = |details| Tone::Lead {
        marker: system.marker_tone.clone(),
        details,
    };
    let mut runs = match (lead.0.is_empty(), details.0.is_empty()) {
        (true, true) => Vec::new(),
        (true, false) => wrapped(details, width, &details_tone),
        (false, true) => wrapped(lead, width, &lead_tone(None)),
        (false, false) if system.details_apart => {
            let mut runs = wrapped(lead, width, &lead_tone(None));
            runs.extend(wrapped(details, width, &details_tone));
            runs
        }
        (false, false) => {
            let from = lead.0.len() + 1;
            let mut marks = lead.1;
            marks.extend(details.1);
            let mut runs = wrapped(
                (format!("{} {}", lead.0, details.0), marks),
                width,
                &details_tone,
            );
            if let Some(first) = runs.first_mut() {
                let details =
                    (first.text.len() > from).then(|| (from, system.details_tone.clone()));
                first.tone = lead_tone(details);
            }
            runs
        }
    };
    // What's wrapped of a lead too long for its row reads as the lead does, muted.
    for run in runs.iter_mut().skip(1) {
        if matches!(run.tone, Tone::Lead { .. }) {
            run.tone = Tone::Status;
        }
    }
    if system.paths_listed {
        runs.extend(system.paths.iter().take(SHOWN_PATHS).map(|path| Run {
            text: fit(path, width),
            tone: Tone::Path,
            links: Vec::new(),
        }));
        if let Some(hidden) = system
            .paths
            .len()
            .checked_sub(SHOWN_PATHS)
            .filter(|n| *n > 0)
        {
            runs.push(Run {
                text: show_more_paths_label(hidden),
                tone: Tone::More,
                links: Vec::new(),
            });
        }
    }
    runs
}

/// People coming and going, or a note: one quiet line, its marker in front when it has
/// one, and no time, as every app draws it (the user's choice, 20 Nov 2026).
fn quiet_rows(look: &Look<'_>, line: &Line, rows: &mut Vec<Row>) {
    let glyph = line
        .system
        .as_ref()
        .map(|system| system.glyph.clone())
        .unwrap_or_default();
    let mut runs = wrapped(
        (line.text.clone(), Vec::new()),
        look.text_width,
        &Tone::Status,
    )
    .into_iter();
    let first = runs.next().map(|run| run.text).unwrap_or_default();
    rows.push(Row::System {
        time: String::new(),
        glyph,
        text: first,
        id: line_id(line).map(str::to_owned),
        agent: None,
        links: Vec::new(),
        mention: false,
    });
    rows.extend(runs.map(|run| run.row(line, String::new())));
}

/// An agent's routine work as one quiet line, zirc-chat's [`SystemLine::brief`], its mark
/// in front; or a card's update said further down, [`Line::text`]: its marker, its words cut
/// to the row and its time.
fn brief_rows(look: &Look<'_>, line: &Line, rows: &mut Vec<Row>) {
    let Some(system) = &line.system else {
        return quiet_rows(look, line, rows);
    };
    let brief = line.shape == RowShape::Brief;
    let agent = line
        .speaker
        .as_ref()
        .filter(|_| brief)
        .and_then(agent)
        .map(|agent| Agent {
            detail: String::new(),
            ..agent
        });
    let text = match &system.brief {
        Some(words) if brief => words.clone(),
        _ => line.text.clone(),
    };
    let mark = if agent.is_some() { AGENT_MARK_WIDTH } else { 0 };
    let width = look
        .text_width
        .saturating_sub(mark + line.clock.width() + 3);
    rows.push(Row::System {
        time: line.clock.clone(),
        glyph: system.glyph.clone(),
        text: fit(&text, width),
        id: line_id(line).map(str::to_owned),
        agent,
        links: Vec::new(),
        mention: false,
    });
}

/// `line`'s words: its blocks ([`Line::rich`]), or its text when it's one plain paragraph.
fn words(look: &Look<'_>, line: &Line, width: usize, tone: &Tone) -> Vec<Run> {
    if line.rich.is_empty() {
        wrapped((line.text.clone(), Vec::new()), width, tone)
    } else {
        blocks(look, &line.rich, width, tone)
    }
}

/// A shared file: its name, its size as zirc-chat says it and where it's fetched, which
/// terminals make clickable.
fn file_runs(file: &LineFile, width: usize) -> Vec<Run> {
    let text = match &file.url {
        Some(url) => format!("{} ({}) {url}", file.name, file.size_text),
        None => format!("{} ({})", file.name, file.size_text),
    };
    wrapped((text, Vec::new()), width, &Tone::Message)
}

/// `runs`, `line`'s words, saying it was corrected after them when it was ([`EDITED`]): on
/// their last row when it fits there, else on a row of its own.
fn edited(line: &Line, runs: &mut Vec<Run>, width: usize) {
    if !line.edited {
        return;
    }
    match runs.last_mut() {
        Some(last) if last.text.width() + 1 + EDITED.width() <= width => {
            last.text = format!("{} {EDITED}", last.text);
        }
        _ => runs.push(Run {
            text: EDITED.to_owned(),
            tone: Tone::More,
            links: Vec::new(),
        }),
    }
}

/// A long message's control, as zirc-chat words it for a terminal
/// ([`zirc_core::chat::lines::LineMore::terminal_line`]): `… 34 more lines · Enter shows them`.
fn more_row(line: &Line) -> Option<Row> {
    let more = line.more.as_ref()?;
    Some(
        Run {
            text: more.terminal_line(),
            tone: Tone::More,
            links: Vec::new(),
        }
        .row(line, String::new()),
    )
}

/// What a run of spans says, and the links and names it shows with where each goes: a link
/// by the words zirc-chat gives it where the terminal shows `links` as labels, else its
/// address as written; an agent session or a person named, as a chip, going nowhere.
fn spans_text(spans: &[RichSpan], links: Links) -> (String, Vec<(String, Option<String>)>) {
    let mut text = String::new();
    let mut marks = Vec::new();
    for span in spans {
        match &span.kind {
            RichSpanKind::Link { url } if !url.chars().any(char::is_control) => match links {
                Links::Labelled => {
                    text.push_str(&span.text);
                    marks.push((span.text.clone(), Some(url.clone())));
                }
                // Code linked to, a file or a commit, keeps its words; a bare address is its
                // address; a link with words of its own says where it goes after them.
                Links::Plain if span.code.is_some() || span.text == *url => {
                    text.push_str(&span.text);
                }
                Links::Plain if span.text == link_label(url) => text.push_str(url),
                Links::Plain => {
                    text.push_str(&span.text);
                    text.push_str(" (");
                    text.push_str(url);
                    text.push(')');
                }
            },
            RichSpanKind::Session { .. } | RichSpanKind::Mention { .. } => {
                text.push_str(&span.text);
                marks.push((span.text.clone(), None));
            }
            _ => text.push_str(&span.text),
        }
    }
    (text, marks)
}

/// `text`, with the links it shows, `marks`, wrapped to `width` in rows that read as `tone`.
fn wrapped(
    (text, marks): (String, Vec<(String, Option<String>)>),
    width: usize,
    tone: &Tone,
) -> Vec<Run> {
    let rows = wrap(&text, width);
    let links = fragments(&rows, &marks, width);
    rows.into_iter()
        .zip(links)
        .map(|(text, links)| Run {
            text,
            tone: tone.clone(),
            links,
        })
        .collect()
}

/// `blocks`, a message's words as zirc-chat gives them ([`RichBlock`]), as rows `width`
/// wide, words reading as `tone`.
fn blocks(look: &Look<'_>, blocks: &[RichBlock], width: usize, tone: &Tone) -> Vec<Run> {
    let mut runs = Vec::new();
    for block in blocks {
        block_runs(look, block, width, tone, &mut runs);
    }
    runs
}

/// `block`'s rows, `width` wide, onto `runs`: words wrapped, a heading bold, a code block's
/// lines indented and cut, never wrapped, a table in columns, a quote behind a bar, a list's
/// items after their markers, a card's updates each on its row and a rule across.
fn block_runs(look: &Look<'_>, block: &RichBlock, width: usize, tone: &Tone, runs: &mut Vec<Run>) {
    match block {
        RichBlock::Paragraph { spans } => {
            runs.extend(wrapped(spans_text(spans, look.links), width, tone));
        }
        RichBlock::Heading { spans, .. } => {
            runs.extend(wrapped(
                spans_text(spans, look.links),
                width,
                &Tone::Heading,
            ));
        }
        RichBlock::Code { text, .. } => {
            let text = text.strip_suffix('\n').unwrap_or(text);
            runs.extend(text.split('\n').map(|code| Run {
                text: fit(&format!("  {code}"), width),
                tone: Tone::Code,
                links: Vec::new(),
            }));
        }
        RichBlock::Table { align, head, rows } => {
            let cells = |row: &zirc_core::chat::RichRow| {
                row.cells
                    .iter()
                    .map(|cell| spans_text(&cell.spans, look.links))
                    .collect::<Vec<_>>()
            };
            let head = cells(head);
            let rows: Vec<_> = rows.iter().map(cells).collect();
            runs.extend(table(align, &head, &rows, width, tone));
        }
        RichBlock::Rule => runs.push(Run {
            text: "─".repeat(width),
            tone: Tone::Status,
            links: Vec::new(),
        }),
        RichBlock::Quote { blocks: quoted } => {
            for run in blocks(look, quoted, width.saturating_sub(2).max(1), &Tone::Quote) {
                runs.push(Run {
                    text: format!("│ {}", run.text),
                    ..run
                });
            }
        }
        RichBlock::Items { items, .. } => {
            for item in items {
                let lead = item.marker.width() + 1;
                let inner = blocks(look, &item.blocks, width.saturating_sub(lead).max(1), tone);
                for (at, run) in inner.into_iter().enumerate() {
                    let before = if at == 0 {
                        format!("{} ", item.marker)
                    } else {
                        " ".repeat(lead)
                    };
                    runs.push(Run {
                        text: format!("{before}{}", run.text),
                        ..run
                    });
                }
            }
        }
        RichBlock::Trail { stubs } => {
            for stub in stubs {
                runs.extend(wrapped(
                    spans_text(&stub.spans, look.links),
                    width,
                    &Tone::Status,
                ));
            }
        }
    }
}

/// A cell's words and the links it shows.
type Cell = (String, Vec<(String, Option<String>)>);

/// A table's rows, `head` over `rows`, in columns, as zirc's other apps draw it as a grid:
/// each column as wide as its widest cell, lined up as zirc-chat says (`align`), `│` between
/// them and `─┼─` under the header; each row cut to `width`.
fn table(
    align: &[CellAlign],
    head: &[Cell],
    rows: &[Vec<Cell>],
    width: usize,
    tone: &Tone,
) -> Vec<Run> {
    fn words(row: &[Cell], at: usize) -> &str {
        row.get(at).map_or("", |cell| cell.0.as_str())
    }
    let all = || std::iter::once(head).chain(rows.iter().map(Vec::as_slice));
    let columns = all().map(<[Cell]>::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|at| all().map(|row| words(row, at).width()).max().unwrap_or(0))
        .collect();
    let line = |row: &[Cell], tone: Tone| {
        let cells: Vec<String> = widths
            .iter()
            .enumerate()
            .map(|(at, &column)| {
                let said = words(row, at);
                let room = column.saturating_sub(said.width());
                let before = match align.get(at).copied().unwrap_or_default() {
                    CellAlign::Start => 0,
                    CellAlign::Center => room / 2,
                    CellAlign::End => room,
                };
                format!("{}{said}{}", " ".repeat(before), " ".repeat(room - before))
            })
            .collect();
        let text = fit(cells.join(" │ ").trim_end(), width);
        let marks: Vec<_> = row.iter().flat_map(|cell| cell.1.clone()).collect();
        let links = fragments(std::slice::from_ref(&text), &marks, width)
            .pop()
            .unwrap_or_default();
        Run { text, tone, links }
    };
    let rule: Vec<String> = widths.iter().map(|&column| "─".repeat(column)).collect();
    let mut runs = vec![
        line(head, Tone::Heading),
        Run {
            text: fit(&rule.join("─┼─"), width),
            tone: Tone::Status,
            links: Vec::new(),
        },
    ];
    runs.extend(rows.iter().map(|row| line(row, tone.clone())));
    runs
}

/// The reactions to `line`, in rows of at most `width` columns.
fn reaction_rows(line: &Line, width: usize) -> Vec<Row> {
    let Some(id) = line_id(line) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    let mut chips: Vec<Chip> = Vec::new();
    let mut used = 0;
    for chip in &line.reactions {
        let wide = chip_text(chip).width();
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
        chips.push(LineReaction::clone(chip));
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
    use chrono::TimeZone as _;
    use zirc_core::chat::{Link as Linked, Options};

    fn unpressed(_: &str) -> Option<String> {
        None
    }

    fn noon() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 30, 12, 0, 0).unwrap()
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    /// Us as `me`, connected at noon on 30 October and in `#api` with alice and bob, and
    /// the machines these tests hear from, whose their hosts say they are.
    fn chat() -> Chat {
        let mut chat = Chat::new("me", "irc.zirc.test", Vec::new(), Options::default());
        chat.clock = noon;
        let _ = chat.connected(Linked {
            nick: "me".into(),
            metadata: false,
            chathistory: false,
            echo: true,
            read_marker: false,
            trim: true,
            multiline: None,
        });
        hear(
            &mut chat,
            &[
                ":me!u@h JOIN #api".to_owned(),
                ":s 353 me = #api :me alice bob mattias-mbp dana-mbp".to_owned(),
                ":s 366 me #api :End".to_owned(),
            ],
        );
        chat
    }

    fn hear(chat: &mut Chat, lines: &[String]) {
        for line in lines {
            let _ = chat.receive(line.parse().unwrap());
        }
    }

    /// `line`, from `who`, said at `time` (`2026-10-30T09:00`) with the id `id`.
    fn at(time: &str, id: &str, who: &str, line: &str) -> String {
        format!("@time={time}:00.000Z;msgid={id} :{who} {line}")
    }

    /// `event` from the agent session `cc-12` on `machine`, owned by `owner`, in `room`.
    fn zirc(
        (time, id): (&str, &str),
        (machine, owner): (&str, &str),
        room: &str,
        event: zirc_proto::Event,
    ) -> String {
        let body = zirc_proto::encode(&zirc_proto::Message::new("cc-12", event)).unwrap();
        at(
            time,
            id,
            &format!("{machine}!u@machine/{owner}"),
            &format!("PRIVMSG {room} :{body}"),
        )
    }

    fn look(chat: &Chat, width: usize, links: Links) -> Look<'_> {
        Look::new(chat, &unpressed, chat.divider(), width, links)
    }

    fn rows_of(chat: &Chat, name: &str, width: usize) -> Vec<Row> {
        layout(
            &look(chat, width, Links::Plain),
            &chat.lines(name, utc(), false),
        )
    }

    fn shown(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Divider(day) => format!("--- {day}"),
                Row::New => "--- New".to_owned(),
                Row::Buttons { labels, said, .. } => {
                    let said = said.as_deref().unwrap_or_default();
                    format!("{:13} {} {said}", "", labels.join(" · "))
                        .trim_end()
                        .to_owned()
                }
                Row::System {
                    time, glyph, text, ..
                } => format!("{time:5} {glyph:>7} {text}").trim_end().to_owned(),
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
                    nick, text, hint, ..
                } => format!("{:5} {nick:>7} {text}  {hint}", "")
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
        let mut chat = chat();
        hear(
            &mut chat,
            &[
                at(
                    "2026-10-30T09:00",
                    "m1",
                    "alice!u@h",
                    "PRIVMSG #api :shipped it",
                ),
                at("2026-10-30T09:01", "m2", "bob!u@h", "PRIVMSG #api :nice"),
                "@+draft/react=🎉;+draft/reply=m1 :bob!u@h TAGMSG #api".to_owned(),
                "@+draft/react=🎉;+draft/reply=m1 :me!u@h TAGMSG #api".to_owned(),
                "@+draft/react=🚀;+draft/reply=m1 :bob!u@h TAGMSG #api".to_owned(),
            ],
        );
        let rows = rows_of(&chat, "#api", 80);
        assert_eq!(
            shown(&rows),
            [
                "--- Today",
                "alice 09:00",
                "              shipped it",
                "              [🎉 2*] [🚀 1]",
                "bob 09:01",
                "              nice",
            ]
        );
        assert_eq!(message_ids(&rows), ["m1", "m2"]);
        // Too many for a row go on to the next.
        let narrow = rows_of(&chat, "#api", 14);
        assert_eq!(
            shown(&narrow)[3..5],
            ["              [🎉 2*]", "              [🚀 1]"]
        );
        // Where each is, for clicks.
        let Row::Reactions { chips, .. } = &rows[3] else {
            panic!("{rows:?}")
        };
        assert_eq!(chip_at(chips, 0).map(|c| c.emoji.as_str()), Some("🎉"));
        assert_eq!(chip_at(chips, 6), None, "the gap");
        assert_eq!(chip_at(chips, 7).map(|c| c.emoji.as_str()), Some("🚀"));
    }

    #[test]
    fn groups_runs_and_divides_days_as_zirc_chat_says() {
        let mut chat = chat();
        hear(
            &mut chat,
            &[
                at("2026-10-29T23:50", "a0", "alice!u@h", "PRIVMSG #api :late"),
                at(
                    "2026-10-30T09:00",
                    "a1",
                    "alice!u@h",
                    "PRIVMSG #api :morning",
                ),
                at("2026-10-30T09:00", "a2", "alice!u@h", "PRIVMSG #api :again"),
                at(
                    "2026-10-30T09:03",
                    "a3",
                    "alice!u@h",
                    "PRIVMSG #api :and again",
                ),
                at("2026-10-30T09:04", "b1", "bob!u@h", "PRIVMSG #api :hi"),
                at(
                    "2026-10-30T09:30",
                    "b2",
                    "bob!u@h",
                    "PRIVMSG #api :much later",
                ),
                "@time=2026-10-30T09:31:00.000Z :carol!u@h JOIN #api".to_owned(),
            ],
        );
        // The core's runs and its days; people coming and going as one quiet line by its
        // marker, with no time (the user's choice, 20 Nov 2026).
        assert_eq!(
            shown(&rows_of(&chat, "#api", 80)),
            [
                "--- Yesterday",
                "alice 23:50",
                "              late",
                "--- Today",
                "alice 09:00",
                "              morning",
                "              again",
                "              and again",
                "bob 09:04",
                "              hi",
                "bob 09:30",
                "              much later",
                "            + carol joined",
            ]
        );
    }

    #[test]
    fn wraps_long_text_under_its_header() {
        let mut chat = chat();
        hear(
            &mut chat,
            &[at(
                "2026-10-30T09:01",
                "m1",
                "alice!u@h",
                "PRIVMSG #api :one two three four five",
            )],
        );
        // 20 less the indent, 2, leaves 18 columns of text.
        assert_eq!(
            shown(&rows_of(&chat, "#api", 20)),
            [
                "--- Today",
                "alice 09:01",
                "              one two three four",
                "              five",
            ]
        );
    }

    fn commit(sha: &str) -> zirc_proto::Event {
        zirc_proto::Event::Commit {
            paths: Vec::new(),
            branch: "main".into(),
            sha: sha.into(),
            subject: "Fix".into(),
            repo: None,
        }
    }

    #[test]
    fn says_a_commit_and_its_push_in_one_row_in_the_agents_own_conversation() {
        let mut chat = chat();
        let push = zirc_proto::Event::Push {
            branch: "main".into(),
            head: "abc1234".into(),
            forced: false,
            subjects: Vec::new(),
            paths: Vec::new(),
            base: None,
            lines: std::collections::BTreeMap::new(),
            repo: None,
            on: None,
        };
        let machine = ("mattias-mbp", "mattias");
        hear(
            &mut chat,
            &[
                zirc(("2026-10-30T09:00", "e1"), machine, "#api", commit("abc")),
                zirc(("2026-10-30T09:01", "e2"), machine, "#api", push),
            ],
        );
        let lines = chat.agent_lines("mattias-mbp/cc-12", utc());
        let rows = layout(&look(&chat, 80, Links::Plain), &lines);
        // One message: its head, the room it was said in after its time, then what it was
        // under it, as zirc-chat folds it.
        let shown = shown(&rows);
        assert_eq!(shown[1], "cc-12 09:00  #api", "{shown:#?}");
        assert!(
            shown[2].contains("committed and pushed to main"),
            "{shown:#?}"
        );
        assert!(matches!(&rows[1], Row::Header { agent: Some(_), .. }));
    }

    #[test]
    fn draws_a_new_divider_above_the_first_unread_without_breaking_the_run() {
        let mut chat = chat();
        hear(
            &mut chat,
            &[
                at("2026-10-30T09:00", "a1", "alice!u@h", "PRIVMSG #api :a1"),
                at("2026-10-30T09:01", "a2", "alice!u@h", "PRIVMSG #api :a2"),
            ],
        );
        let lines = chat.lines("#api", utc(), false);
        let look = Look::new(&chat, &unpressed, Some("a2"), 80, Links::Plain);
        assert_eq!(
            shown(&layout(&look, &lines)),
            [
                "--- Today",
                "alice 09:00",
                "              a1",
                "--- New",
                // Still alice's run: no second header for her.
                "              a2",
            ]
        );
    }

    #[test]
    fn shows_an_apps_posts_as_cards_with_the_outcome_in_front() {
        let mut chat = chat();
        let post = |time: &str, id: &str, text: &str| {
            let landing = zirc_proto::Event::custom(zirc_proto::ci::LANDING_KIND, text);
            let body = zirc_proto::encode(&zirc_proto::Message::new("ci", landing)).unwrap();
            at(time, id, "zirc-ci!u@h", &format!("PRIVMSG #api :{body}"))
        };
        hear(
            &mut chat,
            &[
                post(
                    "2026-10-30T09:00",
                    "c1",
                    "cc-cdbba7: landed web-look as 3d5fe8a (build 435)",
                ),
                post(
                    "2026-10-30T09:01",
                    "c2",
                    "cc-cdbba7: build 436 failed in test: //crates/zirc-tui:zirc-tui_test",
                ),
            ],
        );
        let rows = rows_of(&chat, "#api", 100);
        let shown = shown(&rows);
        // An app by its tag, as zirc's other apps show it; each landing's outcome in front.
        assert_eq!(shown[1], "CI APP 09:00", "{shown:#?}");
        assert!(shown[2].contains("✓ "), "{shown:#?}");
        assert!(shown.iter().any(|row| row.contains("✗ ")), "{shown:#?}");
        assert!(rows.iter().any(|row| matches!(
            row,
            Row::Text {
                tone: Tone::App(Some(Icon::Landed)),
                ..
            }
        )));
    }

    fn intent(paths: usize) -> zirc_proto::Event {
        zirc_proto::Event::Intent {
            summary: "split the login form".into(),
            paths: (1..=paths).map(|n| format!("web/p{n}.ts")).collect(),
            expires: None,
            lasts: None,
        }
    }

    #[test]
    fn says_routine_work_in_a_channel_on_one_quiet_line() {
        let mut chat = chat();
        hear(
            &mut chat,
            &[
                zirc(
                    ("2026-10-30T14:03", "e1"),
                    ("dana-mbp", "dana"),
                    "#api",
                    intent(6),
                ),
                "@time=2026-10-30T14:04:00.000Z :bob!u@h PART #api".to_owned(),
            ],
        );
        let rows = rows_of(&chat, "#api", 60);
        // The core's brief, its time after it: who, what and the first file's name, no
        // paths after it (the user's choice, 14 Nov 2026).
        assert_eq!(
            shown(&rows)[1..],
            [
                "14:03       ✎ cc-12 is about to change p1.ts and 5 more",
                "            − bob left",
            ]
        );
        assert!(matches!(&rows[1], Row::System { agent: Some(_), .. }));
    }

    #[test]
    fn heads_an_event_as_a_message_and_starts_its_body_with_the_lead() {
        // In the agent's own conversation, where its work is whole; in a channel it's one
        // quiet line (says_routine_work_in_a_channel_on_one_quiet_line).
        let mut chat = chat();
        hear(
            &mut chat,
            &[zirc(
                ("2026-10-30T14:03", "e1"),
                ("dana-mbp", "dana"),
                "#api",
                intent(10),
            )],
        );
        let lines = chat.agent_lines("dana-mbp/cc-12", utc());
        let rows = layout(&look(&chat, 60, Links::Plain), &lines);
        let mut expected = vec![
            "--- Today".to_owned(),
            // An agent's activity as a message: its head, who and when; then zirc-chat's
            // lead, its details under it as its paths are listed, and its paths.
            "cc-12 14:03  #api".to_owned(),
            "              ✎ is about to change".to_owned(),
            "              split the login form".to_owned(),
        ];
        expected.extend((1..=SHOWN_PATHS).map(|n| format!("              web/p{n}.ts")));
        expected.push(format!("              {}", show_more_paths_label(2)));
        assert_eq!(shown(&rows), expected);
        assert!(matches!(
            &rows[2],
            Row::Text {
                tone: Tone::Lead { details: None, .. },
                ..
            }
        ));
        assert!(matches!(
            &rows[3],
            Row::Text {
                tone: Tone::Details(_),
                ..
            }
        ));
        assert!(matches!(
            &rows[4],
            Row::Text {
                tone: Tone::Path,
                ..
            }
        ));
        assert!(matches!(
            rows.last(),
            Some(Row::Text {
                tone: Tone::More,
                ..
            })
        ));
    }

    #[test]
    fn draws_a_messages_blocks() {
        let chat = chat();
        let said = [
            "# Plan",
            "",
            "builds:",
            "",
            "| target | took |",
            "| :-: | --: |",
            "| `//web:e2e` | 90s |",
            "| 漢字 | |",
            "",
            "```",
            "let a = 1;",
            "```",
            "",
            "> quoted",
            "",
            "- one",
            "- two",
            "",
            "---",
        ]
        .join("\n");
        let blocks = chat.flat(chat.drawn(zirc_core::chat::rich(&said)));
        let look = look(&chat, 22, Links::Plain);
        let texts: Vec<String> = super::blocks(&look, &blocks, 20, &Tone::Message)
            .into_iter()
            .map(|run| run.text)
            .collect();
        assert_eq!(
            texts,
            [
                "Plan",
                "builds:",
                // Inline code as its words, which zirc-chat gives without their backticks.
                " target   │ took",
                "──────────┼─────",
                "//web:e2e │  90s",
                "  漢字    │",
                "  let a = 1;",
                "│ quoted",
                "• one",
                "• two",
                "────────────────────",
            ]
        );
    }

    #[test]
    fn shows_links_by_their_labels_or_as_written_and_names_people_either_way() {
        let chat = chat();
        let spans = |text: &str| match chat
            .flat(chat.drawn(zirc_core::chat::rich(text)))
            .into_iter()
            .next()
        {
            Some(RichBlock::Paragraph { spans }) => spans,
            other => panic!("{other:?}"),
        };
        let url = "https://github.com/zircapp/zirc/pull/12?tab=files";
        let bare = spans(&format!("see {url} please"));
        assert_eq!(
            spans_text(&bare, Links::Labelled),
            (
                "see github.com/zircapp/zirc/pull/12 please".to_owned(),
                vec![(
                    "github.com/zircapp/zirc/pull/12".to_owned(),
                    Some(url.to_owned())
                )]
            )
        );
        assert_eq!(
            spans_text(&bare, Links::Plain),
            (format!("see {url} please"), Vec::new())
        );
        // A link with words of its own says where it goes after them, where the terminal
        // can't link them.
        let titled = spans(&format!("[the PR]({url})"));
        assert_eq!(
            spans_text(&titled, Links::Plain).0,
            format!("the PR ({url})")
        );
        assert_eq!(spans_text(&titled, Links::Labelled).0, "the PR");
        // People named go by what they show as, a chip that goes nowhere.
        let named = spans("ask @alice");
        assert_eq!(
            spans_text(&named, Links::Plain),
            ("ask alice".to_owned(), vec![("alice".to_owned(), None)])
        );
    }

    #[test]
    fn quotes_what_a_reply_answers_and_says_it_was_edited() {
        let mut chat = chat();
        hear(
            &mut chat,
            &[
                at(
                    "2026-10-30T09:00",
                    "m1",
                    "alice!u@h",
                    "PRIVMSG #api :the login bug",
                ),
                at("2026-10-30T09:01", "m2", "bob!u@h", "PRIVMSG #api :on it")
                    .replace(";msgid=m2", ";msgid=m2;+draft/reply=m1"),
                at("2026-10-30T09:02", "m3", "bob!u@h", "PRIVMSG #api :fixed"),
                at(
                    "2026-10-30T09:03",
                    "m4",
                    "bob!u@h",
                    "PRIVMSG #api :fixed it",
                )
                .replace(";msgid=m4", ";msgid=m4;+draft/edit=m3"),
            ],
        );
        let shown = shown(&rows_of(&chat, "#api", 60));
        assert_eq!(
            shown[3..],
            [
                "bob 09:01".to_owned(),
                "              ↪ alice: the login bug".to_owned(),
                "              on it".to_owned(),
                format!("              fixed it {EDITED}"),
            ],
            "{shown:#?}"
        );
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
        let mut chat = chat();
        hear(
            &mut chat,
            &[
                at(
                    "2026-10-30T09:00",
                    "m1",
                    "alice!u@h",
                    "PRIVMSG #api :morning",
                ),
                at(
                    "2026-10-30T09:01",
                    "m2",
                    "alice!u@h",
                    "PRIVMSG #api :can you look at the login bug? it's been failing since",
                ),
                "@time=2026-10-30T09:02:00.000Z :carol!u@h JOIN #api".to_owned(),
                at("2026-10-30T09:03", "m3", "carol!u@h", "PRIVMSG #api :on it"),
            ],
        );
        let rows = rows_of(&chat, "#api", 40);
        let shown: Vec<String> = rows
            .iter()
            .map(|row| match row {
                Row::Header { nick, time, .. } => format!("{nick} {time}"),
                Row::Text { nick, text, .. } => format!("{nick:2}{text}"),
                Row::System { glyph, text, .. } => format!("{glyph} {text}"),
                Row::Divider(day) => format!("--- {day}"),
                _ => String::new(),
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
                "+ carol joined",
                "carol 09:03",
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
