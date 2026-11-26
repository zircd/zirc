//! A message's text as blocks and spans to draw: Markdown as chat uses it, read once here
//! so every app draws the same thing. Bold, italic, strike, inline code, fenced code blocks
//! with a language, headings, tables, rules, quotes, lists and task lists, links (written
//! or bare), `@mentions` and `#channels`; IRC's own formatting codes map to the same spans.
//! Anything else, such as HTML or an indented code block, stays as it was typed, which is
//! how plain IRC clients show it.

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// A block of a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// Words, line breaks kept.
    Paragraph(Vec<Span>),
    /// A fenced code block, drawn in a monospace box.
    Code {
        /// The language its fence names, such as `rust`, if any.
        language: Option<String>,
        /// Its text, without the last line break.
        text: String,
    },
    /// A heading, `#` to `######`.
    Heading {
        /// How many `#`s: 1 to 6.
        level: u8,
        /// Its words.
        spans: Vec<Span>,
    },
    /// A table: a header row, then its rows, each a list of cells.
    Table {
        /// How each column's cells line up.
        align: Vec<CellAlign>,
        /// The header row's cells.
        head: Vec<Vec<Span>>,
        /// The other rows, each its cells.
        rows: Vec<Vec<Vec<Span>>>,
    },
    /// A rule, `---`: a line across.
    Rule,
    /// A quote, drawn with a bar beside it.
    Quote(Vec<Block>),
    /// A list.
    List {
        /// The first number, for a numbered list.
        start: Option<u64>,
        /// Its items.
        items: Vec<ListItem>,
    },
}

/// An item of a [`Block::List`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListItem {
    /// For a task list's item, `- [ ]` or `- [x]`: whether it's done.
    pub task: Option<bool>,
    /// Its blocks.
    pub blocks: Vec<Block>,
}

/// How a table column's cells line up, as its `:--`, `:-:` or `--:` says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum CellAlign {
    /// From the start, as text does when the column doesn't say.
    #[default]
    Start,
    /// Centred.
    Center,
    /// To the end, as numbers do.
    End,
}

impl CellAlign {
    fn of(alignment: pulldown_cmark::Alignment) -> Self {
        match alignment {
            pulldown_cmark::Alignment::None | pulldown_cmark::Alignment::Left => Self::Start,
            pulldown_cmark::Alignment::Center => Self::Center,
            pulldown_cmark::Alignment::Right => Self::End,
        }
    }
}

/// A run of text drawn one way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Span {
    /// What it says.
    pub text: String,
    /// What it is.
    pub kind: SpanKind,
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Struck through.
    pub strike: bool,
}

/// What a [`Span`] is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SpanKind {
    /// Words.
    #[default]
    Text,
    /// Inline code, in monospace.
    Code,
    /// A link to the address it holds.
    Link(String),
    /// `@nick`: the nick, without the `@`.
    Mention(String),
    /// `#channel`: the channel, with its `#`.
    Channel(String),
    /// An agent session named by its id, `cc-598aa1` or `@cc-598aa1`: the id, which apps
    /// show as a chip with the session's name ([`super::Chat::session_label`]).
    Session(String),
}

impl super::Chat {
    /// What `span` shows when it isn't its own words, as every app draws it: an agent
    /// session or a person named by what they go by ([`super::Chat::kind_label`]), plain,
    /// styled as a mention, with no `@`; a bare link by its host and path ([`link_label`]).
    /// `None` for anything else, which shows as written. The span keeps what it is, the
    /// nick or the id, for its hover and where a click goes.
    #[must_use]
    pub fn span_label(&self, span: &Span) -> Option<String> {
        match &span.kind {
            SpanKind::Link(url) if span.text == *url => Some(link_label(url)),
            kind => self.kind_label(kind),
        }
    }

    /// What a mention shows: a person, `@alice`, by the name they show as, `Alice Liddell`
    /// ([`super::Chat::shown_name`]); an agent session by what it goes by
    /// ([`super::Chat::session_label`]). Never with its `@`. `None` for anything else.
    #[must_use]
    pub fn kind_label(&self, kind: &SpanKind) -> Option<String> {
        match kind {
            SpanKind::Session(id) => Some(self.session_label(id)),
            SpanKind::Mention(nick) => Some(self.shown_name(nick)),
            _ => None,
        }
    }

    /// `text` as plain words, as an excerpt shows it where there's no room for chips, such
    /// as Activity's: formatting gone, and each mention and session by the name it shows
    /// as ([`super::Chat::span_label`]), `Henrik Sandberg the plan is up` for `@henrik the plan is
    /// up`. Blocks on lines of their own.
    #[must_use]
    pub fn shown_text(&self, text: &str) -> String {
        fn flat(blocks: &[Block], out: &mut Vec<String>) {
            for block in blocks {
                match block {
                    Block::Paragraph(spans) | Block::Heading { spans, .. } => {
                        out.push(words(spans));
                    }
                    Block::Code { text, .. } => out.push(text.clone()),
                    Block::Table { head, rows, .. } => {
                        for row in std::iter::once(head).chain(rows) {
                            out.push(
                                row.iter()
                                    .map(|cell| words(cell))
                                    .collect::<Vec<_>>()
                                    .join(" · "),
                            );
                        }
                    }
                    Block::Rule => {}
                    Block::Quote(inner) => flat(inner, out),
                    Block::List { items, .. } => {
                        for item in items {
                            flat(&item.blocks, out);
                        }
                    }
                }
            }
        }
        fn words(spans: &[Span]) -> String {
            spans.iter().map(|span| span.text.as_str()).collect()
        }
        let mut lines = Vec::new();
        flat(&self.drawn(rich(text)), &mut lines);
        lines.join("\n")
    }

    /// `blocks` ready to draw: each span showing what [`super::Chat::span_label`] says,
    /// sessions by their names and bare links by their labels. Every app draws this. A
    /// word naming a session we know, by any id its tool gives it (`codex-7f2a`, not only
    /// `cc-598aa1`), is that session's chip too (the user's choice, 26 Nov 2026).
    #[must_use]
    pub fn drawn(&self, blocks: Vec<Block>) -> Vec<Block> {
        map_spans(blocks, &mut |spans| {
            spans
                .into_iter()
                .flat_map(|span| self.known_sessions(span))
                .map(|mut span| {
                    if let Some(label) = self.span_label(&span) {
                        span.text = label;
                    }
                    span
                })
                .collect()
        })
    }
}

impl super::Chat {
    /// `span`, a run of words, with each word that names a session we know
    /// ([`super::Chat::knows_session`]) a [`SpanKind::Session`] of its own, punctuation
    /// around it left as words; any other span as it is.
    fn known_sessions(&self, span: Span) -> Vec<Span> {
        if span.kind != SpanKind::Text || !span.text.contains(|c: char| !c.is_whitespace()) {
            return vec![span];
        }
        let mut out: Vec<Span> = Vec::new();
        let mut words = String::new();
        let push = |out: &mut Vec<Span>, text: &str, kind: SpanKind| {
            if !text.is_empty() {
                out.push(Span {
                    text: text.to_owned(),
                    kind,
                    ..span.clone()
                });
            }
        };
        for word in span.text.split_inclusive(char::is_whitespace) {
            let start = word
                .find(|c: char| !"([\"'<".contains(c))
                .unwrap_or(word.len());
            let (before, rest) = word.split_at(start);
            let end = rest
                .trim_end_matches(|c: char| c.is_whitespace() || ".,;:!?)]>\"'".contains(c))
                .len();
            let (core, after) = rest.split_at(end);
            let id = core.strip_prefix('@').unwrap_or(core);
            if !id.is_empty() && self.knows_session(id) {
                words.push_str(before);
                push(&mut out, &words, SpanKind::Text);
                words.clear();
                push(&mut out, core, SpanKind::Session(id.to_owned()));
                words.push_str(after);
            } else {
                words.push_str(word);
            }
        }
        push(&mut out, &words, SpanKind::Text);
        out
    }
}

/// `blocks` with every run of spans in them, a paragraph's, a heading's or a table cell's,
/// as `each` makes it, quotes and lists followed into; code blocks and rules as they are.
pub fn map_spans(blocks: Vec<Block>, each: &mut impl FnMut(Vec<Span>) -> Vec<Span>) -> Vec<Block> {
    blocks
        .into_iter()
        .map(|block| match block {
            Block::Paragraph(spans) => Block::Paragraph(each(spans)),
            Block::Heading { level, spans } => Block::Heading {
                level,
                spans: each(spans),
            },
            Block::Table { align, head, rows } => Block::Table {
                align,
                head: head.into_iter().map(&mut *each).collect(),
                rows: rows
                    .into_iter()
                    .map(|row| row.into_iter().map(&mut *each).collect())
                    .collect(),
            },
            Block::Quote(inner) => Block::Quote(map_spans(inner, each)),
            Block::List { start, items } => Block::List {
                start,
                items: items
                    .into_iter()
                    .map(|item| ListItem {
                        task: item.task,
                        blocks: map_spans(item.blocks, each),
                    })
                    .collect(),
            },
            other @ (Block::Code { .. } | Block::Rule) => other,
        })
        .collect()
}

/// Every span in `blocks`, quotes, lists and tables followed into, in order; a code block's
/// text as one plain span.
pub fn each_span(blocks: &[Block], each: &mut impl FnMut(&Span)) {
    for block in blocks {
        match block {
            Block::Paragraph(spans) | Block::Heading { spans, .. } => {
                spans.iter().for_each(&mut *each);
            }
            Block::Table { head, rows, .. } => std::iter::once(head)
                .chain(rows)
                .flatten()
                .flatten()
                .for_each(&mut *each),
            Block::Quote(inner) => each_span(inner, each),
            Block::List { items, .. } => {
                for item in items {
                    each_span(&item.blocks, each);
                }
            }
            Block::Code { text, .. } => each(&Span {
                text: text.clone(),
                ..Span::default()
            }),
            Block::Rule => {}
        }
    }
}

/// `text` as blocks: see the module's description.
#[must_use]
pub fn rich(text: &str) -> Vec<Block> {
    let text = &*super::code_ref::fence_pasted(text);
    let mut reader = Reader::default();
    let mut skip_to: Option<usize> = None;
    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
        if skip_to.is_some_and(|end| range.start < end) {
            continue;
        }
        skip_to = None;
        match event {
            // What the subset leaves out stays as typed.
            Event::Start(Tag::HtmlBlock | Tag::CodeBlock(CodeBlockKind::Indented)) => {
                // From the start of its line, so an indented block keeps its indent.
                let line = text[..range.start].rfind('\n').map_or(0, |at| at + 1);
                reader.as_typed(text[line..range.end].trim_end_matches('\n'));
                skip_to = Some(range.end);
            }
            Event::Html(html) | Event::InlineHtml(html) => reader.text(&html),
            Event::Start(tag) => reader.start(tag),
            Event::End(tag) => reader.end(tag),
            Event::Text(words) => reader.text(&words),
            Event::Code(code) => reader.push(Span {
                text: code.into_string(),
                kind: SpanKind::Code,
                ..reader.style()
            }),
            Event::SoftBreak | Event::HardBreak => reader.text("\n"),
            Event::Rule => reader.block(Block::Rule),
            Event::TaskListMarker(done) => reader.task(done),
            _ => reader.text(&text[range]),
        }
    }
    reader.finish()
}

/// A table in a message's text, as typed: for an app that shows the text as it was
/// written, such as the terminal's, to line its columns up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSource {
    /// Where it's written in the text, its last line's break left out.
    pub range: std::ops::Range<usize>,
    /// How each column's cells line up.
    pub align: Vec<CellAlign>,
    /// Its rows, the header first, each cell as typed without the spaces around it.
    pub rows: Vec<Vec<String>>,
}

/// The tables in `text`, in order.
#[must_use]
pub fn tables_in(text: &str) -> Vec<TableSource> {
    let mut tables = Vec::new();
    let mut table: Option<TableSource> = None;
    let mut row = Vec::new();
    for (event, range) in Parser::new_ext(text, Options::ENABLE_TABLES).into_offset_iter() {
        match event {
            Event::Start(Tag::Table(align)) => {
                table = Some(TableSource {
                    range: range.start..text[..range.end].trim_end_matches('\n').len(),
                    align: align.into_iter().map(CellAlign::of).collect(),
                    rows: Vec::new(),
                });
            }
            Event::Start(Tag::TableCell) => row.push(text[range].trim().to_owned()),
            Event::End(TagEnd::TableHead | TagEnd::TableRow) => {
                if let Some(table) = &mut table {
                    table.rows.push(std::mem::take(&mut row));
                }
            }
            Event::End(TagEnd::Table) => tables.extend(table.take()),
            _ => {}
        }
    }
    tables
}

/// Where the reader is.
#[derive(Debug, Default)]
struct Reader {
    /// The blocks read at the top.
    blocks: Vec<Block>,
    /// The containers open, innermost last.
    open: Vec<Container>,
    /// The paragraph being read.
    paragraph: Option<Vec<Span>>,
    /// The code block being read: its language and text.
    code: Option<(Option<String>, String)>,
    bold: usize,
    italic: usize,
    strike: usize,
    link: Option<String>,
    /// The table being read.
    table: Option<Table>,
}

/// A table as it's read.
#[derive(Debug, Default)]
struct Table {
    align: Vec<CellAlign>,
    head: Vec<Vec<Span>>,
    rows: Vec<Vec<Vec<Span>>>,
    /// The row being read.
    row: Vec<Vec<Span>>,
}

#[derive(Debug)]
enum Container {
    Quote(Vec<Block>),
    List(Option<u64>, Vec<ListItem>),
    Item(ListItem),
}

impl Reader {
    fn style(&self) -> Span {
        Span {
            bold: self.bold > 0,
            italic: self.italic > 0,
            strike: self.strike > 0,
            ..Span::default()
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph | Tag::Heading { .. } | Tag::TableCell => {
                self.close_paragraph();
                self.paragraph = Some(Vec::new());
            }
            Tag::Table(align) => {
                self.close_paragraph();
                self.table = Some(Table {
                    align: align.into_iter().map(CellAlign::of).collect(),
                    ..Table::default()
                });
            }
            Tag::CodeBlock(kind) => {
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().map(str::to_owned)
                    }
                    CodeBlockKind::Indented => None,
                };
                self.code = Some((language, String::new()));
            }
            Tag::BlockQuote(_) => self.open_container(Container::Quote(Vec::new())),
            Tag::List(start) => self.open_container(Container::List(start, Vec::new())),
            Tag::Item => self.open.push(Container::Item(ListItem::default())),
            Tag::Strong => self.bold += 1,
            Tag::Emphasis => self.italic += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                self.link = Some(dest_url.into_string());
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.close_paragraph(),
            TagEnd::Heading(level) => {
                let spans = self.paragraph.take().unwrap_or_default();
                self.add(Block::Heading {
                    level: level as u8,
                    spans,
                });
            }
            TagEnd::TableCell => {
                let cell = self.paragraph.take().unwrap_or_default();
                if let Some(table) = &mut self.table {
                    table.row.push(cell);
                }
            }
            TagEnd::TableHead => {
                if let Some(table) = &mut self.table {
                    table.head = std::mem::take(&mut table.row);
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = &mut self.table {
                    let row = std::mem::take(&mut table.row);
                    table.rows.push(row);
                }
            }
            TagEnd::Table => {
                if let Some(Table {
                    align, head, rows, ..
                }) = self.table.take()
                {
                    self.add(Block::Table { align, head, rows });
                }
            }
            TagEnd::CodeBlock => {
                if let Some((language, mut text)) = self.code.take() {
                    if text.ends_with('\n') {
                        text.pop();
                    }
                    // A fence that names no language, around a diff, is a diff's.
                    let language = language
                        .or_else(|| super::code_ref::sniff_language(&text).map(str::to_owned));
                    self.add(Block::Code { language, text });
                }
            }
            TagEnd::BlockQuote(_) => {
                self.close_paragraph();
                if let Some(Container::Quote(blocks)) = self.open.pop() {
                    self.add(Block::Quote(blocks));
                }
            }
            TagEnd::Item => {
                self.close_paragraph();
                if let Some(Container::Item(item)) = self.open.pop()
                    && let Some(Container::List(_, items)) = self.open.last_mut()
                {
                    items.push(item);
                }
            }
            TagEnd::List(_) => {
                if let Some(Container::List(start, items)) = self.open.pop() {
                    self.add(Block::List { start, items });
                }
            }
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link | TagEnd::Image => self.link = None,
            _ => {}
        }
    }

    /// A block of its own, such as a rule.
    fn block(&mut self, block: Block) {
        self.close_paragraph();
        self.add(block);
    }

    /// The item being read is a task's, done or not.
    fn task(&mut self, done: bool) {
        if let Some(Container::Item(item)) = self.open.last_mut() {
            item.task = Some(done);
        }
    }

    /// A block the subset leaves out, as it was typed.
    fn as_typed(&mut self, source: &str) {
        self.close_paragraph();
        self.add(Block::Paragraph(vec![Span {
            text: source.to_owned(),
            ..Span::default()
        }]));
    }

    /// Words, split into links, mentions and channels, with IRC's formatting codes read.
    fn text(&mut self, words: &str) {
        if let Some((_, code)) = &mut self.code {
            code.push_str(words);
            return;
        }
        let mut style = self.style();
        let mut piece = String::new();
        let mut chars = words.chars().peekable();
        while let Some(c) = chars.next() {
            let toggle = match c {
                '\x02' => Some(0),
                '\x1d' => Some(1),
                '\x1e' => Some(2),
                '\x0f' => Some(3),
                '\x03' => Some(4),
                '\x04' => Some(5),
                '\x11' | '\x16' | '\x1f' => Some(6),
                _ => None,
            };
            let Some(toggle) = toggle else {
                piece.push(c);
                continue;
            };
            self.words(&piece, &style);
            piece.clear();
            match toggle {
                0 => style.bold = !style.bold,
                1 => style.italic = !style.italic,
                2 => style.strike = !style.strike,
                3 => style = self.style(),
                // A colour: up to two digits, a comma and up to two more; a hex colour's
                // are six hex digits each. zirc draws neither.
                4 => skip_colour(&mut chars, 2, char::is_ascii_digit),
                5 => skip_colour(&mut chars, 6, char::is_ascii_hexdigit),
                _ => {}
            }
        }
        self.words(&piece, &style);
    }

    /// Words in one style: a link's text, or plain words with any bare links, mentions and
    /// channels in them.
    fn words(&mut self, words: &str, style: &Span) {
        if words.is_empty() {
            return;
        }
        if let Some(link) = &self.link {
            let link = link.clone();
            self.push(Span {
                text: words.to_owned(),
                kind: SpanKind::Link(link),
                ..style.clone()
            });
            return;
        }
        let mut plain = String::new();
        let mut rest = words;
        while !rest.is_empty() {
            let word_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let (word, after) = rest.split_at(word_end);
            match special(word) {
                Some((before, kind, text, trailing)) => {
                    plain.push_str(before);
                    self.plain(&mut plain, style);
                    self.push(Span {
                        text: text.to_owned(),
                        kind,
                        ..style.clone()
                    });
                    plain.push_str(trailing);
                }
                None => plain.push_str(word),
            }
            let space_end = after
                .find(|c: char| !c.is_whitespace())
                .unwrap_or(after.len());
            plain.push_str(&after[..space_end]);
            rest = &after[space_end..];
        }
        self.plain(&mut plain, style);
    }

    fn plain(&mut self, plain: &mut String, style: &Span) {
        if !plain.is_empty() {
            self.push(Span {
                text: std::mem::take(plain),
                ..style.clone()
            });
        }
    }

    /// Adds a span to the paragraph, joining it to the one before when they're drawn alike.
    fn push(&mut self, span: Span) {
        let spans = self.paragraph.get_or_insert_with(Vec::new);
        if let Some(last) = spans.last_mut()
            && last.kind == span.kind
            && (last.bold, last.italic, last.strike) == (span.bold, span.italic, span.strike)
            && matches!(span.kind, SpanKind::Text | SpanKind::Code)
        {
            last.text.push_str(&span.text);
            return;
        }
        spans.push(span);
    }

    fn close_paragraph(&mut self) {
        if let Some(spans) = self.paragraph.take()
            && !spans.is_empty()
        {
            self.add(Block::Paragraph(spans));
        }
    }

    fn open_container(&mut self, container: Container) {
        self.close_paragraph();
        self.open.push(container);
    }

    /// Adds a block where the reader is: in the innermost quote or item, or at the top.
    fn add(&mut self, block: Block) {
        match self.open.last_mut() {
            Some(Container::Quote(blocks) | Container::Item(ListItem { blocks, .. })) => {
                blocks.push(block);
            }
            // A list holds only items; anything else belongs to its last.
            Some(Container::List(_, items)) => match items.last_mut() {
                Some(item) => item.blocks.push(block),
                None => items.push(ListItem {
                    task: None,
                    blocks: vec![block],
                }),
            },
            None => self.blocks.push(block),
        }
    }

    fn finish(mut self) -> Vec<Block> {
        self.close_paragraph();
        while let Some(container) = self.open.pop() {
            match container {
                Container::Quote(blocks) => self.add(Block::Quote(blocks)),
                Container::Item(item) => item.blocks.into_iter().for_each(|block| self.add(block)),
                Container::List(start, items) => self.add(Block::List { start, items }),
            }
        }
        self.blocks
    }
}

/// Skips a colour code's colours: up to `most` `wanted` characters, and after a comma
/// followed by one, up to `most` more.
fn skip_colour(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    most: usize,
    wanted: fn(&char) -> bool,
) {
    for _ in 0..most {
        chars.next_if(wanted);
    }
    let mut ahead = chars.clone();
    if ahead.next() == Some(',') && ahead.peek().is_some_and(wanted) {
        chars.next();
        for _ in 0..most {
            chars.next_if(wanted);
        }
    }
}

impl super::conversation::Entry {
    /// What a message, action or notice says, as blocks to draw: [`rich`] of its text, IRC
    /// formatting included. Empty for any other entry.
    #[must_use]
    pub fn rich(&self) -> Vec<Block> {
        use super::conversation::EntryKind;
        match &self.kind {
            EntryKind::Message(text) | EntryKind::Action(text) | EntryKind::Notice(text) => {
                rich(self.formatted.as_deref().unwrap_or(text))
            }
            // A decision's text is Markdown too.
            EntryKind::Zirc(message) => match &message.event {
                zirc_proto::Event::Custom { text, .. }
                    if zirc_proto::decision::Decision::of(&message.event).is_some() =>
                {
                    rich(text)
                }
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }
}

/// A bare link, `@mention` or `#channel` in `word`, with any punctuation before and after
/// it: `(see https://zirc.dev).` is `(see`… one word at a time, so `(`, the link and `).`.
fn special(word: &str) -> Option<(&str, SpanKind, &str, &str)> {
    let start = word.find(|c: char| !"([\"'<".contains(c))?;
    let (before, rest) = word.split_at(start);
    let end = rest
        .trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '>', '"', '\''])
        .len();
    let (core, trailing) = rest.split_at(end);
    if core.starts_with("https://") || core.starts_with("http://") {
        return Some((before, SpanKind::Link(core.to_owned()), core, trailing));
    }
    if let Some(session) = session_id(core.strip_prefix('@').unwrap_or(core)) {
        return Some((
            before,
            SpanKind::Session(session.to_owned()),
            core,
            trailing,
        ));
    }
    if let Some(url) = bare_address(core) {
        return Some((before, SpanKind::Link(url), core, trailing));
    }
    let named = |name: &str| {
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_alphanumeric() || "-_[]\\`^{}|/".contains(c))
    };
    if let Some(nick) = core.strip_prefix('@')
        && named(nick)
        && before.is_empty()
    {
        return Some((before, SpanKind::Mention(nick.to_owned()), core, trailing));
    }
    if let Some(channel) = core.strip_prefix('#')
        && named(channel)
        && channel.starts_with(char::is_alphabetic)
        && before.is_empty()
    {
        return Some((before, SpanKind::Channel(core.to_owned()), core, trailing));
    }
    None
}

/// The top-level domains a bare address may end in to be a link: the generic ones people
/// use, and the countries' that turn up as sites in chat. A short list rather than the whole
/// public suffix list, which made the web client's code a third bigger. Sorted, for
/// [`slice::binary_search`].
const TOP_LEVEL_DOMAINS: &[&str] = &[
    "ae", "ai", "app", "ar", "at", "au", "be", "bg", "biz", "blog", "br", "by", "ca", "cc", "ch",
    "cl", "cloud", "cn", "co", "com", "cz", "de", "design", "dev", "dk", "edu", "ee", "es", "eu",
    "fi", "fm", "fr", "gg", "gov", "gr", "hk", "hr", "hu", "id", "ie", "il", "in", "info", "int",
    "io", "is", "it", "jp", "kr", "li", "live", "lt", "lu", "lv", "ly", "md", "me", "mil", "mx",
    "my", "net", "news", "nl", "no", "nz", "online", "org", "page", "ph", "pl", "pt", "py", "ro",
    "rs", "ru", "se", "sg", "sh", "si", "site", "sk", "so", "store", "tech", "th", "to", "tr",
    "tv", "tw", "ua", "uk", "us", "vn", "ws", "xyz", "za",
];

/// `word` when it's an agent session's id as Claude Code names them: `cc-` and six letters
/// or digits, `cc-598aa1`.
fn session_id(word: &str) -> Option<&str> {
    let id = word.strip_prefix("cc-")?;
    (id.len() == 6 && id.chars().all(|c| c.is_ascii_alphanumeric())).then_some(word)
}

/// Who a post is addressed to, when it starts by naming an agent session, `cc-598aa1:` or
/// `@cc-598aa1:`, as agents address each other: the session's id, and the text after it.
#[must_use]
pub fn addressed(text: &str) -> Option<(&str, &str)> {
    let start = text.strip_prefix('@').unwrap_or(text);
    let (word, rest) = start.split_once([':', ',', ' '])?;
    let session = session_id(word)?;
    Some((session, rest.trim_start_matches([':', ',']).trim_start()))
}

/// File extensions that are also top-level domains, which a bare word ending in them links
/// only with a path after it: `main.rs` and `README.md` are files, `docs.rs/serde` a site.
const FILE_EXTENSIONS: &[&str] = &[
    "bzl", "c", "cc", "cpp", "go", "h", "js", "json", "kt", "lock", "md", "mov", "py", "rb", "rs",
    "sh", "so", "swift", "toml", "ts", "tsx", "txt", "yaml", "yml", "zip",
];

/// The `https://` link a bare address typed without a scheme means: `www.` anything, or a
/// host under a real public suffix (`zirc.dev`, `github.com/x/y`, `docs.rs/serde`), but
/// never an email, a version or address made of numbers, or a file named like `main.rs`. A
/// host is under a real suffix when its last label is one of [`TOP_LEVEL_DOMAINS`].
fn bare_address(word: &str) -> Option<String> {
    if word.contains('@') {
        return None;
    }
    let host = word.split(['/', '?', '#']).next()?;
    let labels: Vec<&str> = host.split('.').collect();
    let label = |label: &&str| {
        !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_alphanumeric() || c == '-')
    };
    if labels.len() < 2 || !labels.iter().all(label) {
        return None;
    }
    // Versions and addresses: numbers all through.
    if labels
        .iter()
        .all(|label| label.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let lower = host.to_lowercase();
    let www = lower.starts_with("www.");
    let tld = labels.last()?.to_lowercase();
    // `main.rs` is a file; `docs.rs/serde`, with a path, is a site.
    let path = word.len() > host.len();
    if !www && !path && FILE_EXTENSIONS.contains(&tld.as_str()) {
        return None;
    }
    let known = TOP_LEVEL_DOMAINS.binary_search(&tld.as_str()).is_ok();
    (www || known).then(|| format!("https://{word}"))
}

/// The most characters [`link_label`] shows before it leaves out the middle of a path.
pub const LINK_LABEL_CHARS: usize = 40;

/// What pads a chip's words either side: a narrow no-break space, which a line can't break
/// at, as a thin space let it (a chip's ground left alone at a line's end, 25 Nov 2026).
const CHIP_PAD: char = '\u{202F}';
/// What follows each hyphen in a chip's words, so a line doesn't break there: a word joiner.
const CHIP_JOIN: char = '\u{2060}';

/// A chip's words as every app draws them, a mention's or an agent session's: padded either
/// side and kept whole on its line, `cc-ae9ec0` never broken after `cc-`. [`unchipped`] is
/// what copying them gives back.
#[must_use]
pub fn chip_words(text: &str) -> String {
    let joined = text.replace('-', &format!("-{CHIP_JOIN}"));
    format!("{CHIP_PAD}{joined}{CHIP_PAD}")
}

/// `text` without what [`chip_words`] adds: what copying drawn words gives.
#[must_use]
pub fn unchipped(text: &str) -> String {
    text.chars()
        .filter(|c| *c != CHIP_PAD && *c != CHIP_JOIN)
        .collect()
}

/// A link as apps show it, the whole `url` on hover: its host and path, without the scheme,
/// query, fragment or a trailing slash. A longer one keeps its host and as much of the end
/// of its path as fits in [`LINK_LABEL_CHARS`], with `…` for what's left out:
/// `github.com/…/pull/12`.
#[must_use]
pub fn link_label(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let rest = rest
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim_end_matches('/');
    if rest.chars().count() <= LINK_LABEL_CHARS {
        return rest.to_owned();
    }
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let mut tail: Vec<&str> = Vec::new();
    for segment in path.split('/').rev() {
        let mut parts = tail.clone();
        parts.insert(0, segment);
        let label = format!("{host}/…/{}", parts.join("/"));
        if label.chars().count() > LINK_LABEL_CHARS && !tail.is_empty() {
            break;
        }
        tail = parts;
    }
    let label = format!("{host}/…/{}", tail.join("/"));
    if label.chars().count() <= LINK_LABEL_CHARS {
        return label;
    }
    // Even the last part is too long: its start and end.
    let half = LINK_LABEL_CHARS / 2;
    let chars: Vec<char> = label.chars().collect();
    let start: String = chars[..half].iter().collect();
    let end: String = chars[chars.len() - (LINK_LABEL_CHARS - half - 1)..]
        .iter()
        .collect();
    format!("{start}…{end}")
}

/// Whether `url` is a file hosted by the web app at `web`, such as `https://zirc.dev`, which
/// apps open in their own viewer rather than the browser.
#[must_use]
pub fn media_link(url: &str, web: &str) -> bool {
    let url = if url.contains("://") {
        url.to_owned()
    } else {
        format!("https://{url}")
    };
    url.starts_with(&format!(
        "{}{}/",
        web.trim_end_matches('/'),
        zirc_proto::media::UPLOAD_PATH
    ))
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_chip_stays_whole_and_copies_as_written() {
        let drawn = chip_words("cc-ae9ec0");
        assert_eq!(drawn, "\u{202F}cc-\u{2060}ae9ec0\u{202F}");
        assert_eq!(
            unchipped(&format!("on it: {drawn}, x")),
            "on it: cc-ae9ec0, x"
        );
    }

    use super::*;

    /// A decision's text (the house style) reads as a bold title, a quote and a list.
    #[test]
    fn a_decisions_text_draws_as_title_quote_and_bullets() {
        let decision = zirc_proto::decision::Decision {
            by: "mattias".into(),
            quote: "yes, release it\nbut not before Monday".into(),
            about: "the release".into(),
            session: Some("cc-1".into()),
            link: None,
            for_: vec!["login".into()],
            replaces: None,
        };
        let blocks = rich(&decision.text());
        assert_eq!(blocks.len(), 3, "{blocks:?}");
        assert!(matches!(&blocks[0], Block::Paragraph(spans) if spans[0].bold));
        assert!(matches!(&blocks[1], Block::Quote(_)));
        assert!(matches!(&blocks[2], Block::List { items, .. } if items.len() == 2));
    }

    fn text(text: &str) -> Span {
        Span {
            text: text.into(),
            ..Span::default()
        }
    }

    fn styled(text: &str, bold: bool, italic: bool, strike: bool) -> Span {
        Span {
            text: text.into(),
            bold,
            italic,
            strike,
            ..Span::default()
        }
    }

    fn kind(text: &str, kind: SpanKind) -> Span {
        Span {
            text: text.into(),
            kind,
            ..Span::default()
        }
    }

    #[test]
    fn plain_text_is_one_paragraph_with_its_line_breaks() {
        assert_eq!(
            rich("hi there\nsecond line"),
            [Block::Paragraph(vec![text("hi there\nsecond line")])]
        );
        assert!(rich("").is_empty());
    }

    #[test]
    fn reads_bold_italic_strike_and_inline_code() {
        assert_eq!(
            rich("**bold** *it* _it_ ~~gone~~ `code`"),
            [Block::Paragraph(vec![
                styled("bold", true, false, false),
                text(" "),
                styled("it", false, true, false),
                text(" "),
                styled("it", false, true, false),
                text(" "),
                styled("gone", false, false, true),
                text(" "),
                kind("code", SpanKind::Code),
            ])]
        );
    }

    #[test]
    fn reads_fenced_code_with_its_language() {
        assert_eq!(
            rich("look:\n```rust\nfn main() {}\n```\nok"),
            [
                Block::Paragraph(vec![text("look:")]),
                Block::Code {
                    language: Some("rust".into()),
                    text: "fn main() {}".into()
                },
                Block::Paragraph(vec![text("ok")]),
            ]
        );
    }

    #[test]
    fn reads_quotes_and_lists() {
        assert_eq!(
            rich("> quoted\n\n- one\n- two\n\n3. three"),
            [
                Block::Quote(vec![Block::Paragraph(vec![text("quoted")])]),
                Block::List {
                    start: None,
                    items: vec![item(None, "one"), item(None, "two")]
                },
                Block::List {
                    start: Some(3),
                    items: vec![item(None, "three")]
                },
            ]
        );
    }

    #[test]
    fn reads_links_written_and_bare_mentions_and_channels() {
        assert_eq!(
            rich("[docs](https://zirc.dev) or (https://a.test/x). ask @alice in #api, not a#b"),
            [Block::Paragraph(vec![
                kind("docs", SpanKind::Link("https://zirc.dev".into())),
                text(" or ("),
                kind(
                    "https://a.test/x",
                    SpanKind::Link("https://a.test/x".into())
                ),
                text("). ask "),
                kind("@alice", SpanKind::Mention("alice".into())),
                text(" in "),
                kind("#api", SpanKind::Channel("#api".into())),
                text(", not a#b"),
            ])]
        );
    }

    fn item(task: Option<bool>, words: &str) -> ListItem {
        ListItem {
            task,
            blocks: vec![Block::Paragraph(vec![text(words)])],
        }
    }

    #[test]
    fn reads_headings_rules_and_task_lists() {
        assert_eq!(
            rich("# Plan\nsome **words**\n\n---\n\n### Steps\n- [ ] build\n- [x] test"),
            [
                Block::Heading {
                    level: 1,
                    spans: vec![text("Plan")]
                },
                Block::Paragraph(vec![text("some "), styled("words", true, false, false)]),
                Block::Rule,
                Block::Heading {
                    level: 3,
                    spans: vec![text("Steps")]
                },
                Block::List {
                    start: None,
                    items: vec![item(Some(false), "build"), item(Some(true), "test")]
                },
            ]
        );
        // `#channel` at the start of a line is a channel, not a heading.
        assert_eq!(
            rich("#api is down"),
            [Block::Paragraph(vec![
                kind("#api", SpanKind::Channel("#api".into())),
                text(" is down"),
            ])]
        );
    }

    #[test]
    fn reads_a_table_with_its_columns_alignment() {
        assert_eq!(
            rich("| target | took |\n| :-- | --: |\n| `//web:unit` | 12s |\n| @alice | |\n\nafter"),
            [
                Block::Table {
                    align: vec![CellAlign::Start, CellAlign::End],
                    head: vec![vec![text("target")], vec![text("took")]],
                    rows: vec![
                        vec![vec![kind("//web:unit", SpanKind::Code)], vec![text("12s")]],
                        vec![
                            vec![kind("@alice", SpanKind::Mention("alice".into()))],
                            vec![]
                        ],
                    ],
                },
                Block::Paragraph(vec![text("after")]),
            ]
        );
        // Without its line of dashes, it's words.
        assert_eq!(rich("a | b"), [Block::Paragraph(vec![text("a | b")])]);
    }

    #[test]
    fn finds_a_tables_cells_as_typed() {
        let text = "before\n\n| a | **b** |\n|---|:-:|\n| `1` | 2 |\n\nafter";
        let [table] = tables_in(text).try_into().unwrap();
        assert_eq!(
            &text[table.range.clone()],
            "| a | **b** |\n|---|:-:|\n| `1` | 2 |"
        );
        assert_eq!(table.align, [CellAlign::Start, CellAlign::Center]);
        assert_eq!(table.rows, [["a", "**b**"], ["`1`", "2"]]);
    }

    #[test]
    fn leaves_what_the_subset_has_not_as_typed() {
        assert_eq!(
            rich("<b>html</b>"),
            [Block::Paragraph(vec![text("<b>html</b>")])]
        );
        assert_eq!(
            rich("    indented"),
            [Block::Paragraph(vec![text("    indented")])]
        );
    }

    #[test]
    fn reads_irc_formatting_codes_as_the_same_spans() {
        assert_eq!(
            rich("\x02bold\x02 \x0304,12red\x03 \x1ditalic\x0f done"),
            [Block::Paragraph(vec![
                styled("bold", true, false, false),
                text(" red "),
                styled("italic", false, true, false),
                text(" done"),
            ])]
        );
    }

    #[test]
    fn labels_a_link_by_its_host_and_path() {
        assert_eq!(
            link_label("https://buildkite.com/zirc/monorepo/builds/608"),
            "buildkite.com/zirc/monorepo/builds/608"
        );
        assert_eq!(link_label("https://zirc.dev/"), "zirc.dev");
        assert_eq!(link_label("http://example.com/a?b=c#d"), "example.com/a");
        assert_eq!(
            link_label("https://github.com/zircapp/a-rather-long-repository-name/pull/12"),
            "github.com/…/pull/12"
        );
        let long = link_label(&format!("https://example.com/{}", "x".repeat(80)));
        assert_eq!(long.chars().count(), LINK_LABEL_CHARS);
        assert!(
            long.starts_with("example.com/…/xx") && long.contains('…'),
            "{long}"
        );
        for url in [
            "https://buildkite.com/zirc/monorepo/builds/608/jobs/01a1a14e-654b-4659-a24a-eea5fda73c79",
            "https://a.example/b",
        ] {
            assert!(link_label(url).chars().count() <= LINK_LABEL_CHARS, "{url}");
        }
    }

    #[test]
    fn knows_the_web_apps_files() {
        assert!(media_link(
            "https://zirc.dev/api/media/abc",
            "https://zirc.dev/"
        ));
        assert!(!media_link(
            "https://zirc.dev/api/mediaX",
            "https://zirc.dev"
        ));
        assert!(!media_link(
            "https://evil.dev/api/media/abc",
            "https://zirc.dev"
        ));
        assert!(!media_link(
            "https://zirc.dev.evil/api/media/abc",
            "https://zirc.dev"
        ));
    }

    /// The link a word on its own becomes, if any.
    fn link_of(word: &str) -> Option<String> {
        rich(word).into_iter().find_map(|block| match block {
            Block::Paragraph(spans) => spans.into_iter().find_map(|span| match span.kind {
                SpanKind::Link(url) => Some(url),
                _ => None,
            }),
            _ => None,
        })
    }

    #[test]
    fn links_addresses_typed_without_https() {
        for (typed, url) in [
            ("zirc.dev", "https://zirc.dev"),
            ("github.com/x/y", "https://github.com/x/y"),
            ("docs.rs/serde", "https://docs.rs/serde"),
            ("crates.io/crates/psl", "https://crates.io/crates/psl"),
            ("www.example.com", "https://www.example.com"),
            ("www.example.rs", "https://www.example.rs"),
            ("news.bbc.co.uk/sport", "https://news.bbc.co.uk/sport"),
            ("example.io", "https://example.io"),
            ("Example.COM", "https://Example.COM"),
            ("zirc.dev/docs?q=a#b", "https://zirc.dev/docs?q=a#b"),
            ("zirc.dev/docs", "https://zirc.dev/docs"),
            ("https://zirc.dev", "https://zirc.dev"),
        ] {
            assert_eq!(link_of(typed).as_deref(), Some(url), "{typed}");
        }
        // The text stays as typed, its punctuation outside the link.
        let Block::Paragraph(spans) = &rich("see zirc.dev.")[0] else {
            panic!("a paragraph")
        };
        let link = spans
            .iter()
            .find(|span| matches!(span.kind, SpanKind::Link(_)))
            .unwrap();
        assert_eq!(link.text, "zirc.dev");
        assert_eq!(link_label("https://github.com/x/y"), "github.com/x/y");
        assert!(media_link("zirc.dev/api/media/abc", "https://zirc.dev"));
    }

    #[test]
    fn leaves_files_versions_emails_and_numbers_as_text() {
        for typed in [
            "main.rs",
            "README.md",
            "build.sh",
            "app.py",
            "index.ts",
            "lib.js",
            "libz.so",
            "package.json",
            "ci.yaml",
            "Cargo.lock",
            "Cargo.toml",
            "oss/crates/zirc-chat/src/chat/view.rs",
            "land.sh",
            "main.cc",
            "main.go",
            "App.tsx",
            "ci.yml",
            "0.1.22",
            "v0.1.22",
            "10.0.0.1",
            "a@b.com",
            "e.g.",
            "i.e.",
            "file.txt",
            "example.notatld",
            "localhost",
            "com",
            ".com",
            "-x.com",
        ] {
            assert_eq!(link_of(typed), None, "{typed}");
        }
        // Nor inside code.
        assert_eq!(link_of("`zirc.dev`"), None);
        let fenced = rich("```\nzirc.dev\n```");
        assert!(matches!(fenced[0], Block::Code { .. }), "{fenced:?}");
    }

    #[test]
    fn names_agent_sessions_by_their_ids() {
        let Block::Paragraph(spans) = &rich("ask cc-598aa1, or @cc-ad4f7c.")[0] else {
            panic!("a paragraph")
        };
        let sessions: Vec<(&str, &SpanKind)> = spans
            .iter()
            .filter(|span| matches!(span.kind, SpanKind::Session(_)))
            .map(|span| (span.text.as_str(), &span.kind))
            .collect();
        assert_eq!(
            sessions,
            [
                ("cc-598aa1", &SpanKind::Session("cc-598aa1".into())),
                ("@cc-ad4f7c", &SpanKind::Session("cc-ad4f7c".into())),
            ]
        );
        for word in ["cc-12", "cc-1234567", "xcc-598aa1", "cc-59_aa1"] {
            let Block::Paragraph(spans) = &rich(word)[0] else {
                panic!("a paragraph")
            };
            assert!(
                !spans.iter().any(|s| matches!(s.kind, SpanKind::Session(_))),
                "{word}"
            );
        }
    }

    #[test]
    fn tells_who_a_post_is_addressed_to() {
        assert_eq!(
            addressed("cc-598aa1: thanks, done"),
            Some(("cc-598aa1", "thanks, done"))
        );
        assert_eq!(
            addressed("@cc-ad4f7c heads-up"),
            Some(("cc-ad4f7c", "heads-up"))
        );
        assert_eq!(addressed("cc-598aa1, ok"), Some(("cc-598aa1", "ok")));
        assert_eq!(addressed("done: cc-598aa1 landed"), None);
        assert_eq!(addressed("cc-5:"), None);
    }

    #[test]
    fn keeps_its_top_level_domains_sorted_for_searching() {
        assert!(TOP_LEVEL_DOMAINS.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
