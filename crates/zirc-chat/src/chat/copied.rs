//! What a selection across messages reads as when it's copied, as every app that selects
//! text across them copies it (the user chose it on 10 Nov 2026): one message a line,
//! `[09:12] Henrik Sandberg: the words`, its further lines indented under it. Within one message,
//! a copy is its words alone, as selected: there's no one else to tell apart.
//!
//! A message whose words are all selected copies as it was typed, as its menu's Copy text
//! gives it: a link's address, a code block's fence and a list's markers survive the paste.
//! One the selection starts or ends inside copies the part selected, as it's shown (the
//! user chose both on 16 Nov 2026). Names and times are selected with the words, and say
//! nothing twice: a line starts with them however much of them is selected.

/// One message of a copy: its time as the stream shows it, who said it as the stream names
/// them (none for a line of the app's own), and what of it is selected.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CopiedMessage {
    /// `09:12`.
    pub clock: String,
    /// `Henrik Sandberg`; empty for what has no one to say it.
    pub name: String,
    /// What of its words is selected, as they're shown: the end of the first message's and
    /// the start of the last's, when a selection starts or ends inside them.
    pub text: String,
    /// Whether all of its words are selected, which copies them as `typed`.
    pub whole: bool,
    /// Its words as they were typed: a line's `text`.
    pub typed: String,
    /// What of its name and time is selected, as shown; empty when none of them is.
    pub head: String,
}

impl CopiedMessage {
    /// Its words in the copy: as typed when they're all selected, else the part selected.
    fn words(&self) -> &str {
        if self.whole { &self.typed } else { &self.text }
    }
}

/// How a further line of a copied message starts: under its words, not its time.
const INDENT: &str = "    ";

/// `messages` as copied. One message is its words alone, or the part of its name and time
/// selected when none of its words are, or its line when both are. Several are a line each,
/// `[clock] name: words`, further lines indented; one none of whose words are selected, as
/// where a drag ends on a name, has no line.
#[must_use]
pub fn copied(messages: &[CopiedMessage]) -> String {
    match messages {
        [] => String::new(),
        [one] => match (one.head.is_empty(), one.words().is_empty()) {
            (true, _) => one.words().to_owned(),
            (false, true) => one.head.clone(),
            (false, false) => line(one),
        },
        several => several
            .iter()
            .filter(|message| !message.words().trim().is_empty())
            .map(line)
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn line(message: &CopiedMessage) -> String {
    let mut lines = message.words().trim_end_matches('\n').lines();
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
            ..CopiedMessage::default()
        }
    }

    #[test]
    fn one_message_copies_as_its_words_alone() {
        assert_eq!(
            copied(&[said("09:12", "Henrik Sandberg", "heads up")]),
            "heads up"
        );
        assert_eq!(copied(&[]), "");
    }

    #[test]
    fn several_copy_a_line_each_with_their_time_and_who_said_them() {
        let messages = [
            said(
                "09:12",
                "Henrik Sandberg",
                "heads up, our agents are both in the checkout code",
            ),
            said(
                "09:13",
                "Rajesh Iyer",
                "Thanks!\nI'll let mine finish first\n",
            ),
            said("09:14", "", "Andre joined"),
        ];
        assert_eq!(
            copied(&messages),
            "[09:12] Henrik Sandberg: heads up, our agents are both in the checkout code\n\
             [09:13] Rajesh Iyer: Thanks!\n    I'll let mine finish first\n\
             [09:14] Andre joined"
        );
    }

    #[test]
    fn a_message_selected_whole_copies_as_typed_and_a_part_as_shown() {
        let whole = CopiedMessage {
            whole: true,
            typed: "see [the plan](https://zirc.dev/plan)\n- one\n- two".into(),
            ..said("09:12", "Henrik Sandberg", "see the plan\none\ntwo")
        };
        assert_eq!(
            copied(std::slice::from_ref(&whole)),
            "see [the plan](https://zirc.dev/plan)\n- one\n- two"
        );
        let part = CopiedMessage {
            typed: "Thanks! I'll let *mine* finish first".into(),
            ..said("09:13", "Rajesh Iyer", "Thanks! I'll let")
        };
        assert_eq!(
            copied(&[whole, part]),
            "[09:12] Henrik Sandberg: see [the plan](https://zirc.dev/plan)\n    - one\n    - two\n\
             [09:13] Rajesh Iyer: Thanks! I'll let"
        );
    }

    #[test]
    fn a_name_or_a_time_selected_alone_copies_as_itself_and_with_words_as_the_line() {
        let name = CopiedMessage {
            head: "Henrik".into(),
            ..said("09:12", "Henrik Sandberg", "")
        };
        assert_eq!(copied(&[name]), "Henrik");
        let both = CopiedMessage {
            head: "Chen 09:12".into(),
            ..said("09:12", "Henrik Sandberg", "heads up")
        };
        assert_eq!(copied(&[both]), "[09:12] Henrik Sandberg: heads up");
    }

    #[test]
    fn a_message_with_none_of_its_words_selected_has_no_line_among_several() {
        let ended_on_a_name = CopiedMessage {
            head: "Raj".into(),
            ..said("09:13", "Rajesh Iyer", "")
        };
        assert_eq!(
            copied(&[
                said("09:12", "Henrik Sandberg", "heads up"),
                ended_on_a_name
            ]),
            "[09:12] Henrik Sandberg: heads up"
        );
    }
}
