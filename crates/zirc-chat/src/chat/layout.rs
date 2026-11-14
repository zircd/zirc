//! How a conversation's messages are laid out, as the person chose in Settings: comfortable,
//! under their avatar and name with a run of what one sender says grouped under one head,
//! or compact, one line a message as IRC clients show them. Which of a row's avatar, name
//! and time show, and where the time goes, is decided here once, so the desktop app and the
//! Apple apps draw the same rows; how big and how far apart is theirs.

/// The person's choice of layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    /// A run of what one sender says under their avatar, name and time, once.
    #[default]
    Comfortable,
    /// One line a message: its time, who and what, as IRC clients show them.
    Compact,
}

/// Where a row's time shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockAt {
    /// After the name, in the run's head.
    Header,
    /// Only while the pointer is over the row, in the avatar's column, where an app has a
    /// pointer; nowhere otherwise.
    Gutter,
    /// First on the line, before who.
    Leading,
}

/// Which parts of a row's head show: [`Layout::heading`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Heading {
    /// Whether the sender's avatar shows, left of the text.
    pub avatar: bool,
    /// Whether the sender's name shows.
    pub name: bool,
    /// Where the time goes.
    pub clock: ClockAt,
}

/// The Settings switch for [`Layout::Compact`].
pub const COMPACT_SETTING: &str = "Compact: one line a message, as IRC shows them";

impl Layout {
    /// The layout `compact` says: the setting as kept.
    #[must_use]
    pub const fn of(compact: bool) -> Self {
        if compact {
            Self::Compact
        } else {
            Self::Comfortable
        }
    }

    /// Whether it's [`Layout::Compact`].
    #[must_use]
    pub const fn compact(self) -> bool {
        matches!(self, Self::Compact)
    }

    /// The head of a message's or an event's row that `continues` a run of the same
    /// sender's, or starts one. Comfortable, a run's head has the avatar, the name and the
    /// time, and the rows under it none of them. Compact, every row names its sender after
    /// its time, as IRC does, and none has an avatar. (People coming and going are one
    /// quiet line either way, not a head.)
    #[must_use]
    pub const fn heading(self, continues: bool) -> Heading {
        match self {
            Self::Comfortable if continues => Heading {
                avatar: false,
                name: false,
                clock: ClockAt::Gutter,
            },
            Self::Comfortable => Heading {
                avatar: true,
                name: true,
                clock: ClockAt::Header,
            },
            Self::Compact => Heading {
                avatar: false,
                name: true,
                clock: ClockAt::Leading,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comfortable_heads_a_run_once() {
        let head = Layout::Comfortable.heading(false);
        assert!(head.avatar && head.name);
        assert_eq!(head.clock, ClockAt::Header);
        let under = Layout::Comfortable.heading(true);
        assert!(!under.avatar && !under.name, "{under:?}");
        assert_eq!(under.clock, ClockAt::Gutter);
    }

    #[test]
    fn compact_puts_time_and_name_on_every_message() {
        for continues in [false, true] {
            let line = Layout::Compact.heading(continues);
            assert!(!line.avatar && line.name, "{line:?}");
            assert_eq!(line.clock, ClockAt::Leading);
        }
    }

    #[test]
    fn the_setting_says_which() {
        assert_eq!(Layout::of(true), Layout::Compact);
        assert_eq!(Layout::of(false), Layout::default());
        assert!(Layout::Compact.compact() && !Layout::Comfortable.compact());
    }
}
