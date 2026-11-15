//! A message's rich text as every app's records carry it: zirc-chat's [`Block`]s and
//! [`Span`]s with owned, wire-ready data, code blocks highlighted line by line, and the facts
//! a view marks: a mention of us, a link to a file the web app hosts. Apps draw these as
//! they are.

use serde::Serialize;
use ts_rs::TS;

use super::code_ref::{self, CodeRef, Links};
use super::conversation::fold;
use super::rich::{Block, CellAlign, Span, SpanKind};
use super::{Chat, highlight, link_words, media_link};

/// A block of a message's text: words, a code block, a heading, a table, a rule, a quote or
/// a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[ts(rename_all = "camelCase", tag = "kind")]
pub enum RichBlock {
    /// Words, line breaks kept.
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Paragraph {
        /// Its runs of text.
        spans: Vec<RichSpan>,
    },
    /// A fenced code block, in a monospace box.
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Code {
        /// The language its fence names, as `rust`.
        language: Option<String>,
        /// Its text, for Copy code.
        text: String,
        /// Its lines, highlighted, when the app set a [`Highlighter`]
        /// ([`Chat::set_highlighter`]); empty otherwise, for the app to draw `text`.
        lines: Vec<CodeLine>,
    },
    /// A heading, its spans bold. Every app draws two sizes: `large`, for `#` and `##`, at
    /// zirc-theme's `list-title` step, a step above the body's; the rest at the body's.
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Heading {
        /// Whether it's `#` or `##`, a step larger than the body.
        large: bool,
        /// Its runs of text.
        spans: Vec<RichSpan>,
    },
    /// A table: a grid with lines between its cells, the header row on the raised surface
    /// (its spans bold), each cell on one line, that scrolls sideways when it's wider than
    /// the message.
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Table {
        /// How each column's cells line up; every row has a cell for each.
        align: Vec<CellAlign>,
        /// The header row, its spans bold.
        head: RichRow,
        /// The other rows.
        rows: Vec<RichRow>,
    },
    /// A rule: a thin line across.
    Rule,
    /// A quote, with a bar beside it.
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Quote {
        /// What it quotes.
        blocks: Vec<RichBlock>,
    },
    /// A list. Not `List`, which in the Kotlin bindings would hide Kotlin's own.
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Items {
        /// The first number, for a numbered list.
        start: Option<u64>,
        /// Its items.
        items: Vec<RichItem>,
    },
    /// A card's updates under it, oldest first, after its other blocks: each a ticket stub
    /// still attached to the card, with a perforated tear across before it, a dashed line
    /// with a half-circle notch cut into the card's edge at each end (the user's choice, 14
    /// Nov 2026). Every app draws it so.
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Trail {
        /// Its stubs, one an update.
        stubs: Vec<RichStub>,
    },
}

/// A stub of a [`RichBlock::Trail`]: one of a card's updates, `14:02 running → failed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RichStub {
    /// Its runs of text: its time, bold, then what it did.
    pub spans: Vec<RichSpan>,
}

/// An item of a [`RichBlock::Items`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RichItem {
    /// What stands before it, muted: `•`, its number as `3.`, or a task's box, `☐` or `☑`.
    pub marker: String,
    /// For a task list's item, whether it's done; `None` for any other item.
    pub done: Option<bool>,
    /// Its blocks.
    pub blocks: Vec<RichBlock>,
}

/// A row of a [`RichBlock::Table`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RichRow {
    /// Its cells, in order.
    pub cells: Vec<RichCell>,
}

/// A cell of a [`RichRow`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RichCell {
    /// Its runs of text.
    pub spans: Vec<RichSpan>,
}

/// The ballot boxes a task list's items show: not done, done.
pub const TASK_MARKERS: [&str; 2] = ["☐", "☑"];

impl RichBlock {
    /// Every run of spans in `blocks`, in order, to change: a paragraph's, a heading's, a
    /// table cell's, a stub's, quotes' and lists' followed into.
    pub fn each_run_mut(blocks: &mut [Self], each: &mut impl FnMut(&mut Vec<RichSpan>)) {
        for block in blocks {
            match block {
                Self::Paragraph { spans } | Self::Heading { spans, .. } => each(spans),
                Self::Table { head, rows, .. } => {
                    for row in std::iter::once(head).chain(rows) {
                        for cell in &mut row.cells {
                            each(&mut cell.spans);
                        }
                    }
                }
                Self::Quote { blocks } => Self::each_run_mut(blocks, each),
                Self::Items { items, .. } => {
                    for item in items {
                        Self::each_run_mut(&mut item.blocks, each);
                    }
                }
                Self::Trail { stubs } => {
                    for stub in stubs {
                        each(&mut stub.spans);
                    }
                }
                Self::Code { .. } | Self::Rule => {}
            }
        }
    }

    /// Every span in `blocks`, in order, quotes', lists' and tables' followed into; a code
    /// block has none.
    pub fn each_span<'a>(blocks: &'a [Self], each: &mut impl FnMut(&'a RichSpan)) {
        for block in blocks {
            match block {
                Self::Paragraph { spans } | Self::Heading { spans, .. } => {
                    spans.iter().for_each(&mut *each);
                }
                Self::Table { head, rows, .. } => std::iter::once(head)
                    .chain(rows)
                    .flat_map(|row| &row.cells)
                    .flat_map(|cell| &cell.spans)
                    .for_each(&mut *each),
                Self::Quote { blocks } => Self::each_span(blocks, each),
                Self::Items { items, .. } => {
                    for item in items {
                        Self::each_span(&item.blocks, each);
                    }
                }
                Self::Trail { stubs } => stubs
                    .iter()
                    .flat_map(|stub| &stub.spans)
                    .for_each(&mut *each),
                Self::Code { .. } | Self::Rule => {}
            }
        }
    }
}

/// A run of text drawn one way.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is a style or a fact of its own"
)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RichSpan {
    /// What it says, as drawn ([`Chat::drawn`]): a session by its name, a bare link by its
    /// label.
    pub text: String,
    /// What it is.
    pub kind: RichSpanKind,
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Struck through.
    pub strike: bool,
    /// For a mention, whether it's of us, which apps mark stronger.
    pub me: bool,
    /// For a link, whether it's a file the web app hosts, which apps open in their own
    /// viewer.
    pub media: bool,
    /// The code it names, a repository's file or a commit, when it's a link to one: what
    /// a preview card shows on a hover or a long press ([`super::CodeRef`]).
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<CodeRef>,
    /// A sealed part of the message ([`Chat::sealed_parts`]), drawn as a chip: this span's
    /// text, the lock and its words or its value once revealed. A click reveals or hides it
    /// ([`Chat::toggle_sealed_in`]), and the message's menu copies it ([`Chat::sealed_value`]).
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sealed: Option<super::SealedSpan>,
}

/// What a [`RichSpan`] is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[ts(rename_all = "camelCase", tag = "kind")]
pub enum RichSpanKind {
    /// Words.
    Text,
    /// Inline code, in monospace.
    Code,
    /// A link.
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Link {
        /// Where it goes.
        url: String,
    },
    /// An agent session named in the text, `cc-598aa1`: a chip saying `label`, its id on
    /// hover or a long press, that opens the session's card on a click or tap.
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Session {
        /// Its id.
        id: String,
        /// What to call it: its name once known, or else its id.
        label: String,
        /// The nick of the machine it runs on, for its card ([`Chat::session_machine`]):
        /// `None` when that's unknown, and the chip opens nothing.
        machine: Option<String>,
    },
    /// `@nick`.
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Mention {
        /// The nick, without the `@`.
        nick: String,
    },
    /// `#channel`.
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Channel {
        /// The channel, with its `#`.
        name: String,
    },
}

/// A line of a code block, highlighted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CodeLine {
    /// Its runs, each in one role.
    pub tokens: Vec<CodeToken>,
}

/// A run of code in one role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CodeToken {
    /// The code.
    pub text: String,
    /// The theme's role that colours it: `syntax-keyword` and the like, or `body`
    /// ([`super::Role::theme_role`]).
    pub role: String,
}

/// What colours code blocks' lines, as an app opts into it ([`Chat::set_highlighter`]): the
/// language a fence names, if any, and the code, to its lines. [`highlighted_lines`] is
/// zirc-chat's own. Without one, code blocks come with no lines, and an app draws their text.
pub type Highlighter = fn(Option<&str>, &str) -> Vec<CodeLine>;

/// `code`'s lines highlighted as `language` ([`highlight`]), in zirc-theme's roles: what an
/// app passes [`Chat::set_highlighter`] to have code blocks come coloured. It holds syntect's
/// grammars, so the web client, which loads them only once a code block appears, doesn't.
#[must_use]
pub fn highlighted_lines(language: Option<&str>, code: &str) -> Vec<CodeLine> {
    highlight(code, language)
        .into_iter()
        .map(|tokens| CodeLine {
            tokens: tokens
                .into_iter()
                .map(|token| CodeToken {
                    text: token.text,
                    role: token.role.theme_role().to_owned(),
                })
                .collect(),
        })
        .collect()
}

/// Who reads a message: their nick, for mentions of them, the web app, for links to the
/// files it hosts, and what colours its code, if anything.
#[derive(Clone, Copy, Default)]
struct Reader<'a> {
    me: &'a str,
    web: Option<&'a str>,
    highlighter: Option<Highlighter>,
    /// What links code named in the conversation read.
    code: Links<'a>,
    /// What knows which machine runs each agent session named.
    chat: Option<&'a Chat>,
    /// The conversation it was said in, when known: a machine's, or one of its agents',
    /// is where a session runs that no machine says it runs.
    here: Option<&'a str>,
}

/// `spans`, bold: a heading's words, or a table's header row's.
fn bold(spans: Vec<Span>) -> Vec<Span> {
    spans
        .into_iter()
        .map(|span| Span { bold: true, ..span })
        .collect()
}

impl Reader<'_> {
    fn spans(self, spans: Vec<Span>) -> Vec<RichSpan> {
        spans
            .into_iter()
            .flat_map(|span| self.code_spans(span))
            .collect()
    }

    fn row(self, cells: Vec<Vec<Span>>) -> RichRow {
        RichRow {
            cells: cells
                .into_iter()
                .map(|spans| RichCell {
                    spans: self.spans(spans),
                })
                .collect(),
        }
    }

    fn block(self, block: Block) -> RichBlock {
        let blocks = |blocks: Vec<Block>| blocks.into_iter().map(|b| self.block(b)).collect();
        match block {
            Block::Paragraph(spans) => RichBlock::Paragraph {
                spans: self.spans(spans),
            },
            Block::Heading { level, spans } => RichBlock::Heading {
                large: level <= 2,
                spans: self.spans(bold(spans)),
            },
            Block::Table { align, head, rows } => RichBlock::Table {
                align,
                head: self.row(head.into_iter().map(bold).collect()),
                rows: rows.into_iter().map(|row| self.row(row)).collect(),
            },
            Block::Rule => RichBlock::Rule,
            Block::Code { language, text } => RichBlock::Code {
                lines: self
                    .highlighter
                    .map(|highlight| highlight(language.as_deref(), &text))
                    .unwrap_or_default(),
                language,
                text,
            },
            Block::Quote(quoted) => RichBlock::Quote {
                blocks: blocks(quoted),
            },
            Block::List { start, items } => RichBlock::Items {
                start,
                items: items
                    .into_iter()
                    .zip(0..)
                    .map(|(item, at)| RichItem {
                        marker: match (item.task, start) {
                            (Some(done), _) => TASK_MARKERS[usize::from(done)].to_owned(),
                            (None, Some(first)) => format!("{}.", first + at),
                            (None, None) => "•".to_owned(),
                        },
                        done: item.task,
                        blocks: blocks(item.blocks),
                    })
                    .collect(),
            },
        }
    }

    fn span(self, span: Span) -> RichSpan {
        let text = span.text;
        let (me, media) = match &span.kind {
            SpanKind::Mention(nick) => (!self.me.is_empty() && fold(nick) == fold(self.me), false),
            SpanKind::Link(url) => (false, self.web.is_some_and(|web| media_link(url, web))),
            _ => (false, false),
        };
        RichSpan {
            kind: match span.kind {
                SpanKind::Text => RichSpanKind::Text,
                SpanKind::Code => RichSpanKind::Code,
                SpanKind::Link(url) => RichSpanKind::Link { url },
                SpanKind::Mention(nick) => RichSpanKind::Mention { nick },
                SpanKind::Session(id) => RichSpanKind::Session {
                    label: text.clone(),
                    machine: self
                        .chat
                        .and_then(|chat| chat.session_machine(&id, self.here)),
                    id,
                },
                SpanKind::Channel(name) => RichSpanKind::Channel { name },
            },
            text,
            bold: span.bold,
            italic: span.italic,
            strike: span.strike,
            me,
            media,
            code: None,
            sealed: None,
        }
    }

    /// `span`, split where it names code that links here ([`code_ref`]): plain words at
    /// each file or commit in them, inline code when it's all one.
    fn code_spans(self, span: Span) -> Vec<RichSpan> {
        if !self.code.any() {
            return vec![self.span(span)];
        }
        match span.kind {
            SpanKind::Code => {
                let resolved =
                    code_ref::found(span.text.trim()).and_then(|found| self.code.resolve(&found));
                let Some((url, code)) = resolved else {
                    return vec![self.span(span)];
                };
                vec![self.code_span(span, url, code)]
            }
            SpanKind::Text => {
                let mut out = Vec::new();
                let mut at = 0;
                for (start, end, found) in code_ref::find(&span.text) {
                    let Some((url, code)) = self.code.resolve(&found) else {
                        continue;
                    };
                    if start > at {
                        out.push(self.span(Span {
                            text: span.text[at..start].to_owned(),
                            ..span.clone()
                        }));
                    }
                    let piece = Span {
                        text: span.text[start..end].to_owned(),
                        ..span.clone()
                    };
                    out.push(self.code_span(piece, url, code));
                    at = end;
                }
                if out.is_empty() {
                    return vec![self.span(span)];
                }
                if at < span.text.len() {
                    out.push(self.span(Span {
                        text: span.text[at..].to_owned(),
                        ..span
                    }));
                }
                out
            }
            _ => vec![self.span(span)],
        }
    }

    /// `span` as a link to `url`, or as it was where nothing opens it, naming `code`.
    fn code_span(self, span: Span, url: Option<String>, code: CodeRef) -> RichSpan {
        let kind = match url {
            Some(url) => SpanKind::Link(url),
            None => span.kind.clone(),
        };
        RichSpan {
            code: Some(code),
            ..self.span(Span { kind, ..span })
        }
    }
}

/// A block as records carry it, for where there's no [`Chat`] to say who reads it: no
/// mention is of us, no link a hosted file and no code highlighted. [`Chat::flat`] says
/// all three.
impl From<Block> for RichBlock {
    fn from(block: Block) -> Self {
        Reader::default().block(block)
    }
}

/// A span as records carry it: see [`RichBlock`]'s `From<Block>`.
impl From<Span> for RichSpan {
    fn from(span: Span) -> Self {
        Reader::default().span(span)
    }
}

impl Chat {
    /// `blocks`, drawn already ([`Chat::drawn`], [`Chat::body`]), as records carry them,
    /// code highlighted by the app's [`Highlighter`] if it set one, each mention of us and
    /// each link to a file the organisation's files are at ([`Chat::files_web`]) marked.
    #[must_use]
    pub fn flat(&self, blocks: Vec<Block>) -> Vec<RichBlock> {
        let reader = self.reader();
        blocks
            .into_iter()
            .map(|block| reader.block(block))
            .collect()
    }

    /// [`Chat::flat`] for what was said in `conversation`, with each file and commit it
    /// names linked: into the editor where the repository is checked out here, or to its
    /// forge ([`Chat::set_checkouts`], [`Chat::set_editor`]).
    #[must_use]
    pub fn flat_in(&self, conversation: &str, blocks: Vec<Block>) -> Vec<RichBlock> {
        let reader = Reader {
            code: self.code.links(conversation),
            here: Some(conversation),
            ..self.reader()
        };
        blocks
            .into_iter()
            .map(|block| reader.block(block))
            .collect()
    }

    /// Plain `text` with each whole word `links` names, as `(word, address)`, a link to its
    /// address ([`link_words`]): a system line's commit and branch, to their pages on the
    /// forge. Empty when no word of it links, so apps draw the text as it is.
    #[must_use]
    pub fn linked(&self, text: &str, links: &[(String, String)]) -> Vec<RichSpan> {
        let spans = link_words(
            vec![Span {
                text: text.to_owned(),
                ..Span::default()
            }],
            links,
        );
        if spans.iter().all(|span| span.kind == SpanKind::Text) {
            return Vec::new();
        }
        let reader = self.reader();
        spans.into_iter().map(|span| reader.span(span)).collect()
    }

    /// The files and commits `text`, said in `conversation`, names that link somewhere, in
    /// order, as written and where each goes: for an app that draws `text` as it was written,
    /// such as the terminal's, to link them ([`Chat::flat_in`]).
    #[must_use]
    pub fn code_links(&self, conversation: &str, text: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if self.code.links(conversation).any() {
            RichBlock::each_span(
                &self.flat_in(conversation, super::rich(text)),
                &mut |span| {
                    if let RichSpanKind::Link { url } = &span.kind
                        && span.code.is_some()
                    {
                        out.push((span.text.clone(), url.clone()));
                    }
                },
            );
        }
        out
    }

    /// A card's trail as apps draw it, a [`RichBlock::Trail`]: each of `stubs` an update's
    /// words ([`super::trail_stubs`]). `None` for none.
    #[must_use]
    pub fn flat_trail(&self, stubs: Vec<Vec<Span>>) -> Option<RichBlock> {
        let reader = self.reader();
        (!stubs.is_empty()).then(|| RichBlock::Trail {
            stubs: stubs
                .into_iter()
                .map(|spans| RichStub {
                    spans: reader.spans(spans),
                })
                .collect(),
        })
    }

    fn reader(&self) -> Reader<'_> {
        Reader {
            me: self.me(),
            web: self.files_web(),
            highlighter: self.highlighter,
            code: Links::default(),
            chat: Some(self),
            here: None,
        }
    }

    /// Has code blocks' lines coloured by `highlighter` ([`highlighted_lines`] for
    /// zirc-chat's own), or, with `None`, left empty for the app to draw or colour itself.
    pub fn set_highlighter(&mut self, highlighter: Option<Highlighter>) {
        self.highlighter = highlighter;
    }
}

/// Whether `blocks` are one paragraph of plain words, or nothing: what apps draw as plain
/// text, with no blocks.
pub(super) fn is_plain(blocks: &[Block]) -> bool {
    match blocks {
        [Block::Paragraph(spans)] => spans
            .iter()
            .all(|span| span.kind == SpanKind::Text && !span.bold && !span.italic && !span.strike),
        [] => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{Options, rich};

    fn chat() -> Chat {
        Chat::new("me", "irc.zirc.test", Vec::new(), Options::default())
    }

    #[test]
    fn leaves_code_plain_until_an_app_sets_a_highlighter() {
        let blocks = chat().flat(rich("```rust\nfn main() {}\n```"));
        let [RichBlock::Code { text, lines, .. }] = blocks.as_slice() else {
            panic!("one code block: {blocks:?}");
        };
        assert_eq!(text, "fn main() {}");
        assert!(lines.is_empty(), "{lines:?}");
    }

    #[test]
    fn highlights_code_blocks_line_by_line() {
        let mut chat = chat();
        chat.set_highlighter(Some(highlighted_lines));
        let blocks = chat.flat(rich("```rust\nfn main() {}\nlet x = 1;\n```"));
        let [
            RichBlock::Code {
                language,
                text,
                lines,
            },
        ] = blocks.as_slice()
        else {
            panic!("one code block: {blocks:?}");
        };
        assert_eq!(language.as_deref(), Some("rust"));
        assert_eq!(text, "fn main() {}\nlet x = 1;");
        assert_eq!(lines.len(), 2);
        let first = &lines[0].tokens[0];
        assert_eq!(
            (first.text.as_str(), first.role.as_str()),
            ("fn", "syntax-keyword")
        );
        let whole: String = lines[1].tokens.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(whole, "let x = 1;");
    }

    #[test]
    fn marks_mentions_of_us_and_hosted_files() {
        let mut chat = chat();
        chat.set_picture_site(Some("https://zirc.dev/".into()), Some("acme".into()));
        let upload = format!("https://zirc.dev{}/abc", zirc_proto::media::UPLOAD_PATH);
        let text = format!("@me and @bob see {upload} and https://example.com/x");
        let blocks = chat.flat(rich(&text));
        let [RichBlock::Paragraph { spans }] = blocks.as_slice() else {
            panic!("one paragraph: {blocks:?}");
        };
        let me: Vec<(&str, bool)> = spans
            .iter()
            .filter_map(|span| match &span.kind {
                RichSpanKind::Mention { nick } => Some((nick.as_str(), span.me)),
                _ => None,
            })
            .collect();
        assert_eq!(me, [("me", true), ("bob", false)]);
        let media: Vec<bool> = spans
            .iter()
            .filter(|span| matches!(span.kind, RichSpanKind::Link { .. }))
            .map(|span| span.media)
            .collect();
        assert_eq!(media, [true, false]);
        // Without a chat to ask, nothing is ours or hosted.
        let plain: Vec<RichBlock> = rich(&text).into_iter().map(RichBlock::from).collect();
        let [RichBlock::Paragraph { spans }] = plain.as_slice() else {
            panic!("one paragraph");
        };
        assert!(spans.iter().all(|span| !span.me && !span.media));
    }

    #[test]
    fn sizes_headings_and_marks_list_items() {
        let blocks = chat().flat(rich(
            "## Big\n### Small\n\n| a | b |\n| - | :-: |\n| 1 |\n\n---\n\n- [x] done\n- [ ] not\n\n7. seven\n8. eight\n\n- dot",
        ));
        let [
            RichBlock::Heading { large: true, .. },
            RichBlock::Heading { large: false, .. },
            RichBlock::Table { align, head, rows },
            RichBlock::Rule,
            RichBlock::Items { items: tasks, .. },
            RichBlock::Items {
                items: numbered, ..
            },
            RichBlock::Items { items: bullets, .. },
        ] = blocks.as_slice()
        else {
            panic!("two headings, a table, a rule and three lists: {blocks:?}");
        };
        assert_eq!(*align, [CellAlign::Start, CellAlign::Center]);
        assert!(head.cells[0].spans.iter().all(|span| span.bold));
        assert!(rows[0].cells[0].spans.iter().all(|span| !span.bold));
        assert_eq!(head.cells.len(), 2);
        assert_eq!(
            rows[0].cells.len(),
            2,
            "a short row is filled out with empty cells"
        );
        let markers = |items: &[RichItem]| {
            items
                .iter()
                .map(|item| item.marker.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(markers(tasks), ["☑", "☐"]);
        assert_eq!(tasks[0].done, Some(true));
        assert_eq!(markers(numbered), ["7.", "8."]);
        assert_eq!(markers(bullets), ["•"]);
        assert_eq!(bullets[0].done, None);
    }

    #[test]
    fn keeps_quotes_lists_and_styles() {
        let blocks = chat().flat(rich("> **bold** _it_ ~~gone~~\n\n3. one\n4. `two`"));
        let [
            RichBlock::Quote { blocks: quoted },
            RichBlock::Items { start, items },
        ] = blocks.as_slice()
        else {
            panic!("a quote and a list: {blocks:?}");
        };
        let [RichBlock::Paragraph { spans }] = quoted.as_slice() else {
            panic!("a paragraph quoted");
        };
        assert!(spans[0].bold && spans[0].text == "bold");
        assert!(spans.iter().any(|span| span.italic && span.text == "it"));
        assert!(spans.iter().any(|span| span.strike && span.text == "gone"));
        assert_eq!(*start, Some(3));
        assert_eq!(items.len(), 2);
        let [RichBlock::Paragraph { spans }] = items[1].blocks.as_slice() else {
            panic!("an item's paragraph");
        };
        assert_eq!(spans[0].kind, RichSpanKind::Code);
    }

    #[test]
    fn sessions_show_by_the_text_drawn_and_keep_their_id() {
        let span = RichSpan::from(Span {
            text: "checkout".into(),
            kind: SpanKind::Session("cc-1".into()),
            ..Span::default()
        });
        assert_eq!(
            span.kind,
            RichSpanKind::Session {
                id: "cc-1".into(),
                label: "checkout".into(),
                machine: None,
            }
        );
    }

    #[test]
    fn links_words_only_when_one_links() {
        let chat = chat();
        let links = [("abc1234".to_owned(), "https://forge/c/abc1234".to_owned())];
        assert!(chat.linked("pushed main", &links).is_empty());
        let spans = chat.linked("pushed main to abc1234", &links);
        assert_eq!(spans.last().map(|s| s.text.as_str()), Some("abc1234"));
        assert_eq!(
            spans.last().map(|s| &s.kind),
            Some(&RichSpanKind::Link {
                url: "https://forge/c/abc1234".into()
            })
        );
    }

    #[test]
    fn plain_is_one_unstyled_paragraph() {
        assert!(is_plain(&rich("just words\nand more")));
        assert!(is_plain(&[]));
        assert!(!is_plain(&rich("**bold**")));
        assert!(!is_plain(&rich("see https://example.com")));
        assert!(!is_plain(&rich("one\n\ntwo")));
    }
}
