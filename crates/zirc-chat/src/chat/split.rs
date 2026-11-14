//! A message over what the server takes in one, as several that each fit: cut where a
//! reader would pause, at a blank line or a list item, and never inside a code fence, unless
//! one block alone is too long, which is then cut by lines, its fence closed and reopened.

use zirc_line::multiline::{self, Limits};

/// `text` as the messages it goes as under `limits`: itself when it fits, or else pieces
/// that each do, in order. The composer says how many before sending.
#[must_use]
pub fn split_message(text: &str, limits: Limits) -> Vec<String> {
    let fits = |text: &str| multiline::batch("x", "#x", text, limits).is_some();
    if fits(text) {
        return vec![text.to_owned()];
    }
    let mut messages: Vec<String> = Vec::new();
    let mut current = String::new();
    for block in blocks(text) {
        let joined = if current.is_empty() {
            block.text.clone()
        } else {
            format!("{current}\n{}", block.text)
        };
        if fits(joined.trim_matches('\n')) {
            current = joined;
            continue;
        }
        if !current.trim().is_empty() {
            messages.push(current.trim_matches('\n').to_owned());
        }
        if fits(block.text.trim_matches('\n')) {
            current = block.text;
        } else {
            // Too long alone: by lines, as many as fit, inside its fence if it has one.
            let pieces = by_lines(&block, &fits);
            let (last, whole) = pieces
                .split_last()
                .map_or((None, &[][..]), |(l, w)| (Some(l), w));
            messages.extend(whole.iter().cloned());
            current = last.cloned().unwrap_or_default();
        }
    }
    if !current.trim().is_empty() {
        messages.push(current.trim_matches('\n').to_owned());
    }
    messages
}

/// A run of lines that's cut only around, never inside, unless it's too long alone.
struct Block {
    text: String,
    /// For a code block, the line that opens its fence, such as three backticks and `rust`.
    fence: Option<String>,
}

/// `text` cut where a reader would pause: before a line after a blank one, or before a
/// list item, never inside a code fence.
fn blocks(text: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut fence: Option<String> = None;
    let mut blank_before = false;
    for line in text.split('\n') {
        let trimmed = line.trim_start();
        let opens_or_closes = trimmed.starts_with("```") || trimmed.starts_with("~~~");
        let starts_block = fence.is_none()
            && (blocks.is_empty() || blank_before || is_list_item(trimmed) || opens_or_closes);
        if starts_block {
            blocks.push(Block {
                text: line.to_owned(),
                fence: opens_or_closes.then(|| trimmed.to_owned()),
            });
        } else if let Some(block) = blocks.last_mut() {
            block.text.push('\n');
            block.text.push_str(line);
        }
        if opens_or_closes {
            fence = match fence {
                Some(_) => None,
                None => Some(trimmed.to_owned()),
            };
        }
        blank_before = line.trim().is_empty();
    }
    blocks
}

fn is_list_item(line: &str) -> bool {
    if ["- ", "* ", "+ "].iter().any(|mark| line.starts_with(mark)) {
        return true;
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && (line[digits..].starts_with(". ") || line[digits..].starts_with(") "))
}

/// `block` cut by lines into pieces that each fit, a code block's each in its own fence.
fn by_lines(block: &Block, fits: &dyn Fn(&str) -> bool) -> Vec<String> {
    let (open, close) = match &block.fence {
        Some(open) => (format!("{open}\n"), "\n```".to_owned()),
        None => (String::new(), String::new()),
    };
    let mut lines: Vec<&str> = block.text.split('\n').collect();
    if block.fence.is_some() {
        // Its own fences go round every piece instead.
        lines.remove(0);
        if lines
            .last()
            .is_some_and(|l| l.trim_start().starts_with("```") || l.trim_start().starts_with("~~~"))
        {
            lines.pop();
        }
    }
    let mut pieces = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let wrap = |lines: &[&str]| format!("{open}{}{close}", lines.join("\n"));
    for line in lines {
        current.push(line);
        if current.len() > 1 && !fits(&wrap(&current)) {
            current.pop();
            pieces.push(wrap(&current));
            current = vec![line];
        }
    }
    if !current.is_empty() {
        pieces.push(wrap(&current));
    }
    // A line too long for a message alone still goes, cut by the server's batch rules.
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: Limits = Limits {
        max_bytes: 60,
        max_lines: 6,
    };

    #[test]
    fn a_message_that_fits_goes_whole() {
        assert_eq!(split_message("hi\nthere", SMALL), ["hi\nthere"]);
    }

    #[test]
    fn cuts_at_blank_lines_and_list_items() {
        let text =
            "first paragraph is here\n\nsecond paragraph is here\n- one item here\n- two item here";
        let messages = split_message(text, SMALL);
        assert_eq!(
            messages,
            [
                "first paragraph is here\n\nsecond paragraph is here",
                "- one item here\n- two item here"
            ]
        );
        for message in &messages {
            assert!(multiline::batch("x", "#x", message, SMALL).is_some());
        }
    }

    #[test]
    fn never_cuts_inside_a_code_fence_unless_it_must() {
        let text = "look:\n```rust\nlet a = 1;\nlet b = 2;\n```\nafter that, some more words";
        assert_eq!(
            split_message(text, SMALL),
            [
                "look:",
                "```rust\nlet a = 1;\nlet b = 2;\n```\nafter that, some more words"
            ]
        );
        // A code block too long alone: by lines, each piece in its own fence.
        let long = format!(
            "```sh\n{}\n```",
            (0..8)
                .map(|n| format!("echo {n}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let pieces = split_message(&long, SMALL);
        assert!(pieces.len() > 1, "{pieces:?}");
        for piece in &pieces {
            assert!(
                piece.starts_with("```sh\n") && piece.ends_with("\n```"),
                "{piece:?}"
            );
            assert!(
                multiline::batch("x", "#x", piece, SMALL).is_some(),
                "{piece:?}"
            );
        }
    }
}
