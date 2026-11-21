//! Long content, shown in part: a message whose words run over [`PROSE_LINES`] lines, or
//! whose code block runs over [`CODE_LINES`], shows its start in place, then one "Show N
//! more lines" control, as Slack and Discord do with long posts (the design lead's spec,
//! which the user approved on 5 Nov). It never hides a message: its header and first lines
//! always show, in time order, and there's no ▸/▾, so it can't read as the folds the user
//! turned down. What shows, the label and whether it's open are decided here; apps only
//! draw the control.

use std::collections::HashMap;

use super::rich::{Block, Span};

/// How many lines of words a message shows before it's cut: its summary and first bullets.
/// Code blocks don't count against it; they follow [`CODE_LINES`].
pub const PROSE_LINES: usize = 6;

/// How many lines of a code block show before it's cut.
pub const CODE_LINES: usize = 8;

/// A message's blocks as they show: all of them, or its start and a [`More`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    /// The blocks to draw. A cut code block is whole here, so it's highlighted and copied
    /// whole: [`More::code_lines`] says how much of it shows.
    pub blocks: Vec<Block>,
    /// The control under it, for a message long enough to have one.
    pub more: Option<More>,
}

/// The one control of a long message: "Show N more lines" while it's cut, "Show less" while
/// it's open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct More {
    /// How many lines the cut hides, code included.
    pub hidden: usize,
    /// Whether it shows all of itself, with "Show less" at its end.
    pub open: bool,
    /// While it's cut inside a code block: how many of that block's first lines show. The
    /// block is the last of [`Shown::blocks`]; its last line shown fades into the code's
    /// ground, and the control sits in the block's footer beside Copy. `None` when the cut
    /// is in its words, or it's open: the control then has a line of its own under them.
    pub code_lines: Option<usize>,
    /// Whether it folds a card's earlier updates ([`super::Landing::trail`]) rather than a
    /// long message's lines: it then says `Show 4 earlier updates`, and `hidden` counts them.
    pub updates: bool,
    /// How many steps a done card folds into one line ([`super::Landing::folds_steps`]): it
    /// then says `Show 7 steps`, and opening it shows its earlier updates too.
    pub steps: Option<usize>,
}

impl More {
    /// What the control says: `Show 34 more lines`, `Show 1 more line`, `Show less`.
    #[must_use]
    pub fn label(&self) -> String {
        if let Some(steps) = self.steps {
            return match (self.open, steps) {
                (true, _) => "Hide steps".to_owned(),
                (false, 1) => "Show 1 step".to_owned(),
                (false, steps) => format!("Show {steps} steps"),
            };
        }
        if self.updates {
            return match (self.open, self.hidden) {
                (true, _) => "Show fewer updates".to_owned(),
                (false, 1) => "Show 1 earlier update".to_owned(),
                (false, hidden) => format!("Show {hidden} earlier updates"),
            };
        }
        match (self.open, self.hidden) {
            (true, _) => "Show less".to_owned(),
            (false, 1) => "Show 1 more line".to_owned(),
            (false, hidden) => format!("Show {hidden} more lines"),
        }
    }

    /// What a screen reader says for it, naming whose message it is as its header does:
    /// `Show 34 more lines of Maya Chen's agent's message` for `Maya Chen's agent`.
    #[must_use]
    pub fn spoken(&self, whose: &str) -> String {
        let what = if self.updates || self.steps.is_some() {
            "card"
        } else {
            "message"
        };
        format!("{} of {whose}'s {what}", self.label())
    }

    /// The row the terminal client draws for it, dim, in the text column: `… 34 more lines
    /// · Enter shows them`, or `· Enter shows less` at the end of an open one.
    #[must_use]
    pub fn terminal_line(&self) -> String {
        terminal_line(self.open, self.hidden)
    }
}

/// The row the terminal client draws for a long message's control, `open` or with
/// `hidden` lines cut: see [`More::terminal_line`].
#[must_use]
pub fn terminal_line(open: bool, hidden: usize) -> String {
    match (open, hidden) {
        (true, _) => "· Enter shows less".to_owned(),
        (false, 1) => "… 1 more line · Enter shows it".to_owned(),
        (false, hidden) => format!("… {hidden} more lines · Enter shows them"),
    }
}

/// Which long messages the person opened or closed, by their key in the view: what isn't
/// here shows as it starts ([`super::Chat::body`]).
#[derive(Debug, Clone, Default)]
pub(super) struct Opened(HashMap<String, bool>);

impl super::Chat {
    /// A message's `blocks` as every app shows them: drawn ([`super::Chat::drawn`]), and
    /// cut when long, unless the person opened it. `key` is the message's key in the view.
    /// One that `mentions_me` starts open, so a cut never hides what concerns us; so does
    /// one opened from search or a notification ([`super::Chat::open_more`]).
    #[must_use]
    pub fn body(&self, key: &str, blocks: Vec<Block>, mentions_me: bool) -> Shown {
        let blocks = self.drawn(blocks);
        let Some((start, hidden, code_lines)) = cut(&blocks) else {
            return Shown { blocks, more: None };
        };
        let open = self.opened.0.get(key).copied().unwrap_or(mentions_me);
        if open {
            return Shown {
                blocks,
                more: Some(More {
                    hidden,
                    open,
                    code_lines: None,
                    updates: false,
                    steps: None,
                }),
            };
        }
        Shown {
            blocks: start,
            more: Some(More {
                hidden,
                open,
                code_lines,
                updates: false,
                steps: None,
            }),
        }
    }

    /// Opens a long message, or cuts it again: its control was clicked, tapped, or Enter
    /// pressed on it. `open` is whether it was open, as its [`More`] says.
    pub fn toggle_more(&mut self, key: &str, open: bool) {
        self.opened.0.insert(key.to_owned(), !open);
    }

    /// Opens a long message: it was opened from search or a notification, which point at
    /// something in it.
    pub fn open_more(&mut self, key: &str) {
        self.opened.0.insert(key.to_owned(), true);
    }

    /// The control of a card, the card's key in the view being `key`: `None` while all its
    /// updates and steps show anyway. Its earlier updates fold away, and so do its `steps`
    /// once it's done ([`super::Landing::folds_steps`]), until the person opens them, as a
    /// long message's lines do ([`super::Chat::toggle_more`]).
    #[must_use]
    pub fn trail_more(&self, key: &str, updates: usize, steps: Option<usize>) -> Option<More> {
        let hidden = updates.saturating_sub(super::UPDATES_SHOWN);
        (hidden > 0 || steps.is_some()).then(|| More {
            hidden,
            open: self.opened.0.get(key).copied().unwrap_or(false),
            code_lines: None,
            updates: true,
            steps,
        })
    }
}

/// `blocks` cut to their start, when they're long: the blocks that show, how many lines are
/// hidden, and, when the cut is inside a top-level code block (the last that shows), how
/// many of its lines show. `None` when everything fits.
#[must_use]
pub fn cut(blocks: &[Block]) -> Option<(Vec<Block>, usize, Option<usize>)> {
    let mut cutter = Cutter {
        left: PROSE_LINES,
        hidden: 0,
        cut: false,
        code_lines: None,
    };
    let start = cutter.take(blocks, true);
    cutter
        .cut
        .then_some((start, cutter.hidden, cutter.code_lines))
}

/// How many lines `block` takes, as written: a paragraph's line breaks, a code block's
/// lines, a table's rows, header and all, and what a quote or list holds.
fn lines(block: &Block) -> usize {
    match block {
        Block::Paragraph(spans) => {
            1 + spans
                .iter()
                .map(|span| span.text.matches('\n').count())
                .sum::<usize>()
        }
        Block::Code { text, .. } => text.lines().count().max(1),
        Block::Heading { .. } | Block::Rule => 1,
        Block::Table { rows, .. } => 1 + rows.len(),
        Block::Quote(inner) => inner.iter().map(lines).sum(),
        Block::List { items, .. } => items.iter().flat_map(|item| &item.blocks).map(lines).sum(),
    }
}

struct Cutter {
    /// Lines of words still to show.
    left: usize,
    hidden: usize,
    cut: bool,
    code_lines: Option<usize>,
}

impl Cutter {
    /// What of `blocks` shows, counting what doesn't. A code block at the `top` follows
    /// [`CODE_LINES`]; one in a quote or list counts as words, and shows whole or not at
    /// all, as only top-level blocks get a footer.
    fn take(&mut self, blocks: &[Block], top: bool) -> Vec<Block> {
        let mut shown = Vec::new();
        for block in blocks {
            let count = lines(block);
            if self.cut {
                self.hidden += count;
                continue;
            }
            match block {
                Block::Code { .. } if top => {
                    if count > CODE_LINES {
                        self.hidden += count - CODE_LINES;
                        self.code_lines = Some(CODE_LINES);
                        self.cut = true;
                    }
                    shown.push(block.clone());
                }
                Block::Paragraph(spans) if count > self.left => {
                    if self.left > 0 {
                        shown.push(Block::Paragraph(first_lines(spans, self.left)));
                    }
                    self.hidden += count - self.left;
                    self.left = 0;
                    self.cut = true;
                }
                // A table shows whole or not at all, as a code block in a quote does.
                Block::Code { .. } | Block::Table { .. } | Block::Heading { .. } | Block::Rule
                    if count > self.left =>
                {
                    self.hidden += count;
                    self.cut = true;
                }
                Block::Paragraph(_)
                | Block::Code { .. }
                | Block::Table { .. }
                | Block::Heading { .. }
                | Block::Rule => {
                    self.left -= count;
                    shown.push(block.clone());
                }
                Block::Quote(inner) => {
                    let inner = self.take(inner, false);
                    if !inner.is_empty() {
                        shown.push(Block::Quote(inner));
                    }
                }
                Block::List { start, items } => {
                    let mut kept = Vec::new();
                    for item in items {
                        let blocks = self.take(&item.blocks, false);
                        if !blocks.is_empty() {
                            kept.push(super::rich::ListItem {
                                task: item.task,
                                blocks,
                            });
                        }
                    }
                    if !kept.is_empty() {
                        shown.push(Block::List {
                            start: *start,
                            items: kept,
                        });
                    }
                }
            }
        }
        shown
    }
}

/// `spans` up to their `count`th line, without the break that ends it.
fn first_lines(spans: &[Span], count: usize) -> Vec<Span> {
    let mut left = count;
    let mut shown = Vec::new();
    for span in spans {
        let mut text = String::new();
        for (at, line) in span.text.split('\n').enumerate() {
            if at > 0 {
                left -= 1;
                if left == 0 {
                    break;
                }
                text.push('\n');
            }
            text.push_str(line);
        }
        if !text.is_empty() {
            shown.push(Span {
                text,
                ..span.clone()
            });
        }
        if left == 0 {
            break;
        }
    }
    shown
}
