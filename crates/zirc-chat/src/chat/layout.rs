//! How a conversation's messages are laid out: under the sender's avatar and name, with a
//! run of what one sender says grouped under one head. Which of a row's avatar, name and
//! time show, and where the time goes, is decided here once, so the desktop app and the
//! Apple apps draw the same rows; how big and how far apart is theirs. There's one layout:
//! the Compact one, IRC's line a message, went on 19 Nov 2026, the user's choice of one
//! density for every app.

/// Where a row's time shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockAt {
    /// After the name, in the run's head.
    Header,
    /// Only while the pointer is over the row, in the avatar's column, where an app has a
    /// pointer; nowhere otherwise.
    Gutter,
}

/// Which parts of a row's head show: [`row_heading`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Heading {
    /// Whether the sender's avatar shows, left of the text.
    pub avatar: bool,
    /// Whether the sender's name shows.
    pub name: bool,
    /// Where the time goes.
    pub clock: ClockAt,
}

/// The head of a message's or an event's row that `continues` a run of the same sender's,
/// or starts one: a run's head has the avatar, the name and the time, and the rows under it
/// none of them. (People coming and going are one quiet line, not a head.)
#[must_use]
pub const fn row_heading(continues: bool) -> Heading {
    if continues {
        Heading {
            avatar: false,
            name: false,
            clock: ClockAt::Gutter,
        }
    } else {
        Heading {
            avatar: true,
            name: true,
            clock: ClockAt::Header,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_is_headed_once() {
        let head = row_heading(false);
        assert!(head.avatar && head.name);
        assert_eq!(head.clock, ClockAt::Header);
        let under = row_heading(true);
        assert!(!under.avatar && !under.name, "{under:?}");
        assert_eq!(under.clock, ClockAt::Gutter);
    }
}
