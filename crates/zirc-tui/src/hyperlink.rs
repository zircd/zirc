//! Links in messages, clickable where the terminal can: each link shows as the words
//! zirc-chat gives it, a bare address as its label, inside an OSC 8 hyperlink to the whole
//! address. Ratatui has no hyperlinks of its own, and its buffer holds one symbol a cell, so
//! the escapes go into the first and last cells of what's drawn, with their widths forced to
//! what shows. A terminal that doesn't understand OSC 8 shows the address as it was written instead.

use std::num::NonZeroU16;

use ratatui::buffer::{Buffer, CellDiffOption};
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr as _;

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
