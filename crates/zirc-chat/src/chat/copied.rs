//! What several messages read as when they're copied together, as every app copies them
//! (the user chose it on 10 Nov 2026): one message a line, `[09:12] Maya Chen: the words`,
//! its further lines indented under it. Within one message, a copy is its words alone, as
//! selected: there's no one else to tell apart.

/// One message of a copy: its time as the stream shows it, who said it as the stream names
/// them (none for a line of the app's own), and the part of its words selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopiedMessage {
    /// `09:12`.
    pub clock: String,
    /// `Maya Chen`; empty for what has no one to say it.
    pub name: String,
    /// What of its words is selected: all of them, or the end of the first and the start of
    /// the last when a selection starts or ends inside them.
    pub text: String,
}

/// How a further line of a copied message starts: under its words, not its time.
const INDENT: &str = "    ";

/// `messages` as copied: the words alone for one, `[clock] name: words` a line each for
/// several, further lines indented.
#[must_use]
pub fn copied(messages: &[CopiedMessage]) -> String {
    match messages {
        [] => String::new(),
        [one] => one.text.clone(),
        several => several.iter().map(line).collect::<Vec<_>>().join("\n"),
    }
}

fn line(message: &CopiedMessage) -> String {
    let mut lines = message.text.trim_end_matches('\n').lines();
    let first = lines.next().unwrap_or_default();
    let head = if message.name.is_empty() {
        format!("[{}] {first}", message.clock)
    } else {
        format!("[{}] {}: {first}", message.clock, message.name)
    };
    lines.fold(head, |out, more| format!("{out}\n{INDENT}{more}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(clock: &str, name: &str, text: &str) -> CopiedMessage {
        CopiedMessage {
            clock: clock.into(),
            name: name.into(),
            text: text.into(),
        }
    }

    #[test]
    fn one_message_copies_as_its_words_alone() {
        assert_eq!(
            copied(&[said("09:12", "Maya Chen", "heads up")]),
            "heads up"
        );
        assert_eq!(copied(&[]), "");
    }

    #[test]
    fn several_copy_a_line_each_with_their_time_and_who_said_them() {
        let messages = [
            said(
                "09:12",
                "Maya Chen",
                "heads up, our agents are both in the checkout code",
            ),
            said(
                "09:13",
                "Leo Okafor",
                "Thanks!\nI'll let mine finish first\n",
            ),
            said("09:14", "", "Priya joined"),
        ];
        assert_eq!(
            copied(&messages),
            "[09:12] Maya Chen: heads up, our agents are both in the checkout code\n\
             [09:13] Leo Okafor: Thanks!\n    I'll let mine finish first\n\
             [09:14] Priya joined"
        );
    }
}
