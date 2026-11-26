//! What a server logs of what's said on it. A zirc server whose owners turned on debug
//! logging says so in a line of its MOTD, one of [`Logged`]'s notices, which plain IRC
//! clients show as they are and zirc's own clients recognise, to show where people type.

/// What's logged of the messages people send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Logged {
    /// Messages in channels, but not direct ones.
    Channels,
    /// Every message, direct ones too.
    Everything,
}

impl Logged {
    /// The MOTD line that says so.
    #[must_use]
    pub fn notice(self) -> &'static str {
        match self {
            Self::Channels => {
                "Debug logging is on: channel messages here are logged for the organisation's \
                 owners. Direct messages aren't."
            }
            Self::Everything => {
                "Debug logging is on: messages here, direct messages too, are logged for the \
                 organisation's owners."
            }
        }
    }

    /// It in a few words, for a status bar.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Channels => "channel messages are logged for the owners",
            Self::Everything => "all messages are logged for the owners, direct ones too",
        }
    }

    /// What a MOTD line says is logged, if it's one of the notices. The server may put
    /// something before it, such as Ergo's `- `.
    #[must_use]
    pub fn from_motd(line: &str) -> Option<Self> {
        let line = line.trim_end();
        [Self::Channels, Self::Everything]
            .into_iter()
            .find(|logged| line.ends_with(logged.notice()))
    }
}

#[cfg(test)]
mod tests {
    use super::Logged;

    #[test]
    fn notices_are_recognised_in_a_motd_and_nothing_else_is() {
        for logged in [Logged::Channels, Logged::Everything] {
            assert_eq!(Logged::from_motd(logged.notice()), Some(logged));
            assert_eq!(
                Logged::from_motd(&format!("- {}  ", logged.notice())),
                Some(logged)
            );
        }
        assert_eq!(
            Logged::from_motd("zirc · zirc's private server · zirc.dev"),
            None
        );
        assert_eq!(Logged::from_motd("Debug logging is on"), None);
    }
}
