//! Colour for what a person reads at a terminal: `zirc who`, `status`, `sessions` and `lease`.
//! Restrained, a weight or one colour a line, to find a name or a warning at a glance. Only
//! when the output is a terminal and `NO_COLOR` isn't set: piped, as scripts and agents read
//! it, every byte is what it was. The commands write as before; what they wrote is painted a
//! line at a time on its way out ([`Lines`]), so no command knows about colour.

use std::io::{IsTerminal as _, Write};

/// Whether to paint: a terminal reads the output, and nothing asked for none.
pub(crate) fn wanted() -> bool {
    std::io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var_os("TERM").is_none_or(|term| term != "dumb")
}

const BOLD: &str = "1";
const DIM: &str = "2";
const RED: &str = "31";
const GREEN: &str = "32";
const YELLOW: &str = "33";
const CYAN: &str = "36";

/// `text` in the style `code` names.
fn styled(code: &str, text: &str) -> String {
    format!("\x1b[{code}m{text}\x1b[0m")
}

/// `line` with what's before its first `at` in the style `code`, or as it is without one.
fn lead(code: &str, line: &str, at: &str) -> String {
    match line.split_once(at) {
        Some((lead, rest)) => format!("{}{at}{rest}", styled(code, lead)),
        None => line.to_owned(),
    }
}

/// A line of `zirc who`: a channel in bold, a heading under it dim, and who an entry is of
/// in cyan.
pub(crate) fn who(line: &str) -> String {
    let indent = line.len() - line.trim_start().len();
    match indent {
        0 if !line.is_empty() => styled(BOLD, line),
        2 if line.ends_with(':') => styled(DIM, line),
        4 => {
            let (space, entry) = line.split_at(indent);
            format!("{space}{}", lead(CYAN, entry, ": "))
        }
        _ => line.to_owned(),
    }
}

/// A line of `zirc status`: connected in green, and what needs a look in yellow.
pub(crate) fn status(line: &str) -> String {
    // With several organisations, each line starts with its organisation's name.
    let said = line.split_once(": ").map_or(line, |(_, said)| said);
    if said.starts_with("connected to") {
        line.replacen("connected", &styled(GREEN, "connected"), 1)
    } else if said.starts_with("connecting to")
        || said.starts_with("not connecting to")
        || line.starts_with("zircd was busy")
        || line.starts_with("wakes paused")
    {
        styled(YELLOW, line)
    } else {
        line.to_owned()
    }
}

/// A line of `zirc sessions`: a session in bold, and a landing that failed in red.
pub(crate) fn sessions(line: &str) -> String {
    if line.starts_with(' ') {
        if line.contains(" failed") {
            styled(RED, line)
        } else {
            line.to_owned()
        }
    } else {
        lead(BOLD, line, " ")
    }
}

/// A line of `zirc lease`: the lease in bold, and `free` in green.
pub(crate) fn lease(line: &str) -> String {
    let line = lead(BOLD, line, ": ");
    match line.strip_suffix(": free") {
        Some(lease) => format!("{lease}: {}", styled(GREEN, "free")),
        None => line,
    }
}

/// What a command writes, painted a line at a time by `rule` on its way to `out`, when
/// `on`; passed through untouched otherwise.
pub(crate) struct Lines<'a, W: Write> {
    out: &'a mut W,
    rule: fn(&str) -> String,
    on: bool,
    /// The start of a line not ended yet.
    held: Vec<u8>,
}

impl<'a, W: Write> Lines<'a, W> {
    pub(crate) fn new(out: &'a mut W, rule: fn(&str) -> String, on: bool) -> Self {
        Self {
            out,
            rule,
            on,
            held: Vec::new(),
        }
    }
}

impl<W: Write> Write for Lines<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if !self.on {
            return self.out.write(buf);
        }
        self.held.extend_from_slice(buf);
        while let Some(end) = self.held.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.held.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line[..end]);
            writeln!(self.out, "{}", (self.rule)(&line))?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if !self.held.is_empty() {
            let rest = std::mem::take(&mut self.held);
            write!(self.out, "{}", (self.rule)(&String::from_utf8_lossy(&rest)))?;
        }
        self.out.flush()
    }
}

impl<W: Write> Drop for Lines<'_, W> {
    fn drop(&mut self) {
        // What the command's last line left without a newline still goes out.
        let _ = self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piped_every_byte_is_what_the_command_wrote() {
        let mut out = Vec::new();
        {
            let mut lines = Lines::new(&mut out, who, false);
            write!(lines, "#api\n  about to change:\n    mbp/cc-1: src").unwrap();
        }
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "#api\n  about to change:\n    mbp/cc-1: src"
        );
    }

    #[test]
    fn at_a_terminal_each_line_is_painted_however_it_was_written() {
        let mut out = Vec::new();
        {
            let mut lines = Lines::new(&mut out, who, true);
            // A line written in two halves, and a last one without its newline.
            write!(lines, "#a").unwrap();
            write!(lines, "pi\n  about to change:\n").unwrap();
            write!(lines, "    mbp/cc-1 (login): src: the form").unwrap();
        }
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\x1b[1m#api\x1b[0m\n\x1b[2m  about to change:\x1b[0m\n    \
             \x1b[36mmbp/cc-1 (login)\x1b[0m: src: the form"
        );
    }

    #[test]
    fn paints_what_to_find_at_a_glance_and_leaves_the_rest() {
        assert_eq!(
            who("  nobody has uncommitted changes"),
            "  nobody has uncommitted changes"
        );
        assert_eq!(
            status("connected to irc.example:6697"),
            "\x1b[32mconnected\x1b[0m to irc.example:6697"
        );
        assert_eq!(
            status("zirc: connecting to irc.example:6697"),
            "\x1b[33mzirc: connecting to irc.example:6697\x1b[0m"
        );
        assert_eq!(status("nick mbp"), "nick mbp");
        assert_eq!(
            sessions("cc-1 on mbp · 2m ago"),
            "\x1b[1mcc-1\x1b[0m on mbp · 2m ago"
        );
        assert_eq!(
            sessions("  landing: build 7 of login failed: //a:b"),
            "\x1b[31m  landing: build 7 of login failed: //a:b\x1b[0m"
        );
        assert_eq!(
            sessions("  landing: build 7 landed"),
            "  landing: build 7 landed"
        );
        assert_eq!(
            lease("build: free"),
            "\x1b[1mbuild\x1b[0m: \x1b[32mfree\x1b[0m"
        );
        assert_eq!(
            lease("build: cc-1 for 12m more"),
            "\x1b[1mbuild\x1b[0m: cc-1 for 12m more"
        );
        assert_eq!(
            lease("no leases held or waited for"),
            "no leases held or waited for"
        );
    }
}
