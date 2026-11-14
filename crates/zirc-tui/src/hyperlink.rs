//! Links in messages, clickable where the terminal can: each bare address shows as
//! zirc-chat's [`link_label`] inside an OSC 8 hyperlink to the whole address. Ratatui has no
//! hyperlinks of its own, and its buffer holds one symbol a cell, so the escapes go into the
//! first and last cells of what's drawn, with their widths forced to what shows. A terminal
//! that doesn't understand OSC 8 shows the address as it was written instead.

use std::num::NonZeroU16;

use ratatui::buffer::{Buffer, CellDiffOption};
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr as _;
use zirc_core::chat::{Block, SpanKind, each_span, link_label, rich};

/// How the terminal shows links.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Links {
    /// As labels, clickable through OSC 8.
    Labelled,
    /// As written, for a terminal without OSC 8.
    Plain,
}

impl Links {
    /// What the terminal we run in can do, by the environment `var` reads:
    /// `ZIRC_TUI_LINKS=plain` or `=labelled` says; otherwise terminals known to take OSC 8
    /// get labels. Under tmux or screen, which pass the escapes on only when set up to, and
    /// anywhere unknown, links stay as written.
    #[must_use]
    pub fn detect(var: impl Fn(&str) -> Option<String>) -> Self {
        if let Some(said) = var("ZIRC_TUI_LINKS") {
            return if matches!(said.as_str(), "plain" | "off") {
                Self::Plain
            } else {
                Self::Labelled
            };
        }
        if var("TMUX").is_some() || var("STY").is_some() {
            return Self::Plain;
        }
        let program = var("TERM_PROGRAM").unwrap_or_default();
        let term = var("TERM").unwrap_or_default();
        let known = matches!(
            program.as_str(),
            "iTerm.app" | "WezTerm" | "vscode" | "ghostty" | "Hyper" | "Tabby" | "rio"
        ) || ["kitty", "alacritty", "foot", "ghostty", "wezterm", "rio"]
            .iter()
            .any(|name| term.contains(name))
            || var("WT_SESSION").is_some()
            || var("KONSOLE_VERSION").is_some()
            || var("VTE_VERSION")
                .and_then(|version| version.parse::<u32>().ok())
                .is_some_and(|version| version >= 5000);
        if known { Self::Labelled } else { Self::Plain }
    }
}

/// A link drawn on a row of the scrollback, or an agent session named there: what of its
/// label the row shows, all of it or the part that fits, and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// What the row shows of it.
    pub shown: String,
    /// The whole address; `None` for an agent session, drawn as a mention is.
    pub url: Option<String>,
}

/// `text` with each agent session named in it by its name, `zirc-chat` (`session` says
/// what to call one), each person named by their nick without the `@`, and each bare
/// address as its label where the terminal shows `links` so; and those labels with where
/// they go, in order. `code` is the files and commits it names that link, as written and
/// where each goes ([`zirc_core::chat::Chat::code_links`]), which keep their words.
/// Addresses written as Markdown links, or in code, stay as written.
#[must_use]
pub fn label(
    text: &str,
    links: Links,
    names: &dyn Fn(&SpanKind) -> Option<String>,
    code: &[(String, String)],
) -> (String, Vec<(String, Option<String>)>) {
    let mut found = Vec::new();
    named(&rich(text), &mut found);
    // Each in order of where it's written, what it shows and where it goes.
    let mut placed: Vec<(usize, String, String, Option<String>)> = Vec::new();
    let mut from = 0;
    for (written, kind) in found {
        let (label, url) = match kind {
            SpanKind::Link(url) if links == Links::Labelled => (link_label(&url), Some(url)),
            // An agent session or a person named, `@alice`, as zirc-chat names them, a chip
            // with no "@" ([`zirc_core::chat::Chat::kind_label`]).
            SpanKind::Session(_) | SpanKind::Mention(_) => {
                (names(&kind).unwrap_or_else(|| written.clone()), None)
            }
            _ => continue,
        };
        let Some(at) = text[from..].find(&written).map(|at| at + from) else {
            continue;
        };
        from = at + written.len();
        placed.push((at, written, label, url));
    }
    let mut from = 0;
    for (written, url) in code {
        let Some(at) = text[from..].find(written.as_str()).map(|at| at + from) else {
            continue;
        };
        from = at + written.len();
        let overlaps = placed
            .iter()
            .any(|(start, other, ..)| at < start + other.len() && *start < from);
        if !overlaps && !url.chars().any(char::is_control) {
            placed.push((at, written.clone(), written.clone(), Some(url.clone())));
        }
    }
    placed.sort_by_key(|(at, ..)| *at);
    let mut shown = String::new();
    let mut labels = Vec::new();
    let mut at = 0;
    for (start, written, label, url) in placed {
        shown.push_str(&text[at..start]);
        shown.push_str(&label);
        at = start + written.len();
        labels.push((label, url));
    }
    shown.push_str(&text[at..]);
    (shown, labels)
}

/// What's named in `blocks`, as written and what it is: bare addresses, links whose text
/// is their address, and agent sessions. An address with a control character in it, which
/// could end the escape early, is left as text.
fn named(blocks: &[Block], found: &mut Vec<(String, SpanKind)>) {
    // A code block's text comes as plain words, which name nothing.
    each_span(blocks, &mut |span| match &span.kind {
        SpanKind::Link(url) if *url == span.text && !url.chars().any(char::is_control) => {
            found.push((span.text.clone(), span.kind.clone()));
        }
        SpanKind::Session(_) | SpanKind::Mention(_) => {
            found.push((span.text.clone(), span.kind.clone()));
        }
        _ => {}
    });
}

/// The links each of `rows`, wrapped `width` columns wide from text holding `links`, shows.
/// A label too long for a row is cut over several, whole rows of it then the rest at the
/// start of the next, and each part links to the whole address.
#[must_use]
pub fn fragments(
    rows: &[String],
    links: &[(String, Option<String>)],
    width: usize,
) -> Vec<Vec<Link>> {
    let mut shown = vec![Vec::new(); rows.len()];
    for (label, url) in links {
        let mut add = |at: usize, part: &str| {
            shown[at].push(Link {
                shown: part.to_owned(),
                url: url.clone(),
            });
        };
        // What's left of a label cut at the end of the row before.
        let mut rest = "";
        for (at, row) in rows.iter().enumerate() {
            if !rest.is_empty() && !row.is_empty() {
                if let Some(after) = rest.strip_prefix(row.as_str()) {
                    add(at, row);
                    rest = after;
                    continue;
                }
                if row.starts_with(rest) {
                    add(at, rest);
                }
            }
            rest = "";
            if row.contains(label.as_str()) {
                add(at, label);
            } else if !row.is_empty() && row.width() + 2 > width {
                // A full row that a label starts: it was cut there.
                if let Some(after) = label.strip_prefix(row.as_str()) {
                    add(at, row);
                    rest = after;
                }
            }
        }
    }
    shown
}

/// Whether `url` may go into a terminal's hyperlink escape: an http or https address with no
/// control character in it, which would end the escape and start one of its sender's own
/// (the clipboard's, the title's).
fn plain(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://"))
        && !url.chars().any(char::is_control)
}

/// Makes each of `links` drawn on the row `y` of `buffer`, between `area`'s sides, a
/// hyperlink: OSC 8 opens in front of its first cell's symbol and closes after its last's.
/// An agent session's is drawn in `mention` instead.
pub fn mark(buffer: &mut Buffer, area: Rect, y: u16, links: &[Link], mention: Style) {
    if links.is_empty() {
        return;
    }
    // Each cell drawn, where its symbol starts in the row's text, and how wide it is. The
    // cells a wide symbol covers after its own are left out.
    let mut text = String::new();
    let mut cells: Vec<(usize, u16, usize)> = Vec::new();
    let mut x = area.left();
    while x < area.right() {
        let symbol = buffer[(x, y)].symbol();
        let width = symbol.width().max(1);
        cells.push((text.len(), x, width));
        text.push_str(symbol);
        x = x.saturating_add(u16::try_from(width).unwrap_or(1));
    }
    let mut marks: Vec<(u16, String, bool)> = Vec::new();
    for link in links {
        for (start, part) in text.match_indices(&link.shown) {
            let end = start + part.len();
            let Some(first) = cells.iter().position(|(at, ..)| *at == start) else {
                continue;
            };
            let last = cells
                .iter()
                .rposition(|(at, ..)| *at < end)
                .unwrap_or(first);
            let Some(url) = &link.url else {
                for (_, x, _) in &cells[first..=last] {
                    buffer[(*x, y)].set_style(mention);
                }
                continue;
            };
            // Written into the terminal's own escape: one that isn't a plain web address
            // stays text, whoever made the link, as a landing's comes from whoever sent it.
            if !plain(url) {
                continue;
            }
            marks.push((cells[first].1, format!("\x1b]8;;{url}\x1b\\"), true));
            marks.push((cells[last].1, "\x1b]8;;\x1b\\".to_owned(), false));
        }
    }
    for (x, escape, opens) in marks {
        let cell = &mut buffer[(x, y)];
        let symbol = cell.symbol().to_owned();
        let width =
            NonZeroU16::new(u16::try_from(symbol.width()).unwrap_or(1)).unwrap_or(NonZeroU16::MIN);
        let symbol = if opens {
            format!("{escape}{symbol}")
        } else {
            format!("{symbol}{escape}")
        };
        cell.set_symbol(&symbol)
            .set_diff_option(CellDiffOption::ForcedWidth(width));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_web_address_becomes_a_terminal_link() {
        assert!(plain("https://buildkite.com/zirc/monorepo/builds/7"));
        assert!(!plain("x\u{1b}\\\u{1b}]52;c;aGk=\u{7}"));
        assert!(!plain("https://a.example/\u{1b}]52;c;aGk=\u{7}"));
        assert!(!plain("file:///etc/passwd") && !plain("javascript:alert(1)"));
    }

    #[test]
    fn labels_bare_addresses_and_leaves_written_links_and_code() {
        let (shown, links) = label(
            "see https://github.com/zircapp/zirc/pull/12?tab=files and `https://x.dev`",
            Links::Labelled,
            &|_| None,
            &[],
        );
        assert_eq!(
            shown,
            "see github.com/zircapp/zirc/pull/12 and `https://x.dev`"
        );
        assert_eq!(
            links,
            [(
                "github.com/zircapp/zirc/pull/12".to_owned(),
                Some("https://github.com/zircapp/zirc/pull/12?tab=files".to_owned())
            )]
        );
        let (shown, links) = label(
            "[the PR](https://github.com/zircapp/zirc/pull/12)",
            Links::Labelled,
            &|_| None,
            &[],
        );
        assert_eq!(shown, "[the PR](https://github.com/zircapp/zirc/pull/12)");
        assert!(links.is_empty());
    }

    #[test]
    fn links_the_files_and_commits_zirc_chat_found_keeping_their_words() {
        let code = [
            (
                "src/a.rs:4".to_owned(),
                "vscode://file/src/api/src/a.rs:4".to_owned(),
            ),
            (
                "3f2a1c0".to_owned(),
                "https://github.com/acme/api/commit/3f2a1c0".to_owned(),
            ),
        ];
        let (shown, links) = label(
            "src/a.rs:4 broke in 3f2a1c0, see https://zirc.dev/x",
            Links::Labelled,
            &|_| None,
            &code,
        );
        assert_eq!(shown, "src/a.rs:4 broke in 3f2a1c0, see zirc.dev/x");
        assert_eq!(
            links,
            [
                (
                    "src/a.rs:4".to_owned(),
                    Some("vscode://file/src/api/src/a.rs:4".to_owned())
                ),
                (
                    "3f2a1c0".to_owned(),
                    Some("https://github.com/acme/api/commit/3f2a1c0".to_owned())
                ),
                (
                    "zirc.dev/x".to_owned(),
                    Some("https://zirc.dev/x".to_owned())
                ),
            ]
        );
    }

    #[test]
    fn names_agent_sessions_by_what_they_go_by_whatever_the_terminal() {
        let session = |kind: &SpanKind| match kind {
            SpanKind::Session(id) if id == "cc-598aa1" => Some("zirc-chat".to_owned()),
            SpanKind::Session(id) => Some(id.clone()),
            _ => None,
        };
        let (shown, links) = label(
            "ask cc-598aa1 or @cc-ad4f7c, see https://zirc.dev/x",
            Links::Plain,
            &session,
            &[],
        );
        // Plain, the address stays as written; sessions are named either way.
        assert_eq!(shown, "ask zirc-chat or cc-ad4f7c, see https://zirc.dev/x");
        assert_eq!(
            links,
            [
                ("zirc-chat".to_owned(), None),
                ("cc-ad4f7c".to_owned(), None)
            ]
        );
    }

    #[test]
    fn follows_a_label_cut_over_rows() {
        let links = [(
            "zirc.dev/abcdefgh".to_owned(),
            Some("https://zirc.dev/abcdefgh".to_owned()),
        )];
        let rows = crate::rows::wrap("go zirc.dev/abcdefgh now", 8);
        assert_eq!(rows, ["go", "zirc.dev", "/abcdefg", "h now"]);
        let parts: Vec<Vec<String>> = fragments(&rows, &links, 8)
            .into_iter()
            .map(|row| row.into_iter().map(|link| link.shown).collect())
            .collect();
        assert_eq!(
            parts,
            [
                vec![],
                vec!["zirc.dev".to_owned()],
                vec!["/abcdefg".to_owned()],
                vec!["h".to_owned()]
            ]
        );
    }

    #[test]
    fn takes_osc_8_where_the_terminal_is_known_to() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        assert_eq!(
            Links::detect(env(&[("TERM_PROGRAM", "iTerm.app")])),
            Links::Labelled
        );
        assert_eq!(
            Links::detect(env(&[("TERM", "xterm-kitty")])),
            Links::Labelled
        );
        assert_eq!(
            Links::detect(env(&[("TERM_PROGRAM", "Apple_Terminal")])),
            Links::Plain
        );
        assert_eq!(
            Links::detect(env(&[("TERM_PROGRAM", "iTerm.app"), ("TMUX", "/tmp/t")])),
            Links::Plain
        );
        assert_eq!(
            Links::detect(env(&[
                ("TERM_PROGRAM", "Apple_Terminal"),
                ("ZIRC_TUI_LINKS", "on")
            ])),
            Links::Labelled
        );
    }
}
