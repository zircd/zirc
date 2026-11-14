//! Reading a channel's history from the server (`CHATHISTORY`, from IRC version 3) over a
//! range of time, a page at a time: the rules zircd's reads and `//ops:history` share.
//!
//! The server answers at most its `chathistory-maxmessages` (Ergo's default is 1000) per
//! request, so a longer range is read back from its end, each page before the oldest message
//! so far by its id, so none is read twice or skipped, until the range's start, history's
//! start, or the newest `limit` are in.

use zirc_irc::Line;

/// The most messages asked for in one request.
pub const PAGE: usize = 1000;

/// The most pages one read goes back: a safety stop, 500 000 messages. A read that reaches
/// it says so ([`Pages::stopped_short`]).
pub const MAX_PAGES: usize = 500;

/// Which messages: those from `since` to `until` (seconds since the Unix epoch, either
/// open), and of those the newest `limit`, if set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Range {
    /// The oldest moment, inclusive.
    pub since: Option<u64>,
    /// The newest moment, exclusive.
    pub until: Option<u64>,
    /// The most messages, newest kept.
    pub limit: Option<usize>,
}

/// One item of a page, as paging needs it: a message, or one of the joins, parts and quits
/// Ergo keeps in history beside them. Every item anchors the next page, but only messages
/// count towards [`Range::limit`]: a page of joins and quits alone isn't history's start
/// (it was taken for one, and `//ops:history -n 3` answered nothing, on 5 Nov).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seen {
    /// The server's id for it (`msgid`).
    pub id: Option<String>,
    /// When the server says it was sent: its `time` tag.
    pub time: Option<String>,
    /// Whether it's a message (PRIVMSG or NOTICE), not a join, part or quit.
    pub message: bool,
}

impl Seen {
    /// What `line` says of itself.
    #[must_use]
    pub fn of(line: &Line) -> Self {
        Self {
            id: line.tag("msgid").map(str::to_owned),
            time: line.tag("time").map(str::to_owned),
            message: matches!(line.command.as_str(), "PRIVMSG" | "NOTICE"),
        }
    }

    fn at(&self) -> Option<u64> {
        self.time.as_deref().and_then(crate::time::seconds_at)
    }
}

/// Where a read through [`Range`] stands: what to ask for next, and what of each page to
/// keep.
#[derive(Debug, Clone)]
pub struct Pages {
    range: Range,
    /// The anchor of the next request (`msgid=…` or `timestamp=…`), or `None` for the
    /// newest.
    before: Option<String>,
    kept: usize,
    read: usize,
    done: bool,
}

impl Pages {
    /// A read through `range`, from its end.
    #[must_use]
    pub fn new(range: Range) -> Self {
        Self {
            range,
            before: range
                .until
                .map(|until| format!("timestamp={}", crate::time::tag(until))),
            kept: 0,
            read: 0,
            done: false,
        }
    }

    /// The next request for `target`'s history, or `None` once the read is done.
    #[must_use]
    pub fn request(&self, target: &str) -> Option<Line> {
        if self.done || self.read >= MAX_PAGES {
            return None;
        }
        let wanted = self
            .range
            .limit
            .map_or(PAGE, |limit| limit.saturating_sub(self.kept).min(PAGE));
        if wanted == 0 {
            return None;
        }
        let count = wanted.to_string();
        Some(match &self.before {
            Some(anchor) => Line::new("CHATHISTORY", ["BEFORE", target, anchor, &count]),
            None => Line::new("CHATHISTORY", ["LATEST", target, "*", &count]),
        })
    }

    /// Takes in a page, oldest first: how many of its first messages to leave out, as older
    /// than the range. A message without a time is kept: nothing says it's out of range.
    pub fn page(&mut self, page: &[Seen]) -> usize {
        self.read += 1;
        let Some(oldest) = page.first() else {
            self.done = true;
            return 0;
        };
        let next = oldest
            .id
            .as_ref()
            .map(|id| format!("msgid={id}"))
            .or_else(|| oldest.time.as_ref().map(|time| format!("timestamp={time}")));
        let since = self.range.since;
        let older = |seen: &Seen| since.is_some_and(|since| seen.at().is_some_and(|at| at < since));
        let reached = older(oldest);
        let skipped = page.iter().take_while(|seen| older(seen)).count();
        self.kept += page
            .iter()
            .skip(skipped)
            .filter(|seen| seen.message)
            .count();
        if reached || next.is_none() || next == self.before {
            self.done = true;
        }
        self.before = next;
        skipped
    }

    /// What it reads.
    #[must_use]
    pub fn range(&self) -> Range {
        self.range
    }

    /// Whether it stopped at [`MAX_PAGES`] with more history to go.
    #[must_use]
    pub fn stopped_short(&self) -> bool {
        !self.done && self.read >= MAX_PAGES
    }

    /// How many to drop from the oldest of `kept` messages, to leave the newest `limit`.
    #[must_use]
    pub fn excess(&self, kept: usize) -> usize {
        self.range
            .limit
            .map_or(0, |limit| kept.saturating_sub(limit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(id: &str, time: &str) -> Seen {
        Seen {
            id: Some(id.into()),
            time: (!time.is_empty()).then(|| time.into()),
            message: true,
        }
    }

    #[test]
    fn a_page_of_joins_and_quits_alone_reads_on() {
        let mut pages = Pages::new(Range {
            limit: Some(3),
            ..Range::default()
        });
        let quit = |id: &str| Seen {
            message: false,
            ..seen(id, "")
        };
        assert_eq!(pages.page(&[quit("j1"), quit("q1"), quit("j2")]), 0);
        // Not history's start: it asks for what's before the oldest, still for three.
        let next = pages.request("#api").unwrap();
        assert_eq!(next.params, ["BEFORE", "#api", "msgid=j1", "3"]);
        pages.page(&[seen("m1", ""), seen("m2", ""), quit("q0"), seen("m3", "")]);
        assert!(pages.request("#api").is_none(), "three messages are in");
    }

    #[test]
    fn reads_the_newest_once_when_they_fit_a_page() {
        let mut pages = Pages::new(Range {
            limit: Some(20),
            ..Range::default()
        });
        let first = pages.request("#api").unwrap();
        assert_eq!(first.params, ["LATEST", "#api", "*", "20"]);
        let page: Vec<Seen> = (0..20).map(|i| seen(&i.to_string(), "")).collect();
        assert_eq!(pages.page(&page), 0);
        assert!(pages.request("#api").is_none());
    }

    #[test]
    fn pages_back_from_until_to_since() {
        let since = crate::time::seconds_at("2026-11-05T10:00:00Z").unwrap();
        let until = crate::time::seconds_at("2026-11-05T12:00:00Z").unwrap();
        let mut pages = Pages::new(Range {
            since: Some(since),
            until: Some(until),
            limit: None,
        });
        let first = pages.request("#api").unwrap();
        assert_eq!(
            first.params,
            [
                "BEFORE",
                "#api",
                "timestamp=2026-11-05T12:00:00.000Z",
                "1000"
            ]
        );
        assert_eq!(pages.page(&[seen("b", "2026-11-05T11:00:00Z")]), 0);
        let second = pages.request("#api").unwrap();
        assert_eq!(second.params, ["BEFORE", "#api", "msgid=b", "1000"]);
        // The range's start is in this page: the older ones are left out, and it's done.
        let page = [
            seen("x", "2026-11-05T09:00:00Z"),
            seen("y", ""),
            seen("a", "2026-11-05T10:30:00Z"),
        ];
        assert_eq!(pages.page(&page), 1, "a message with no time is kept");
        assert!(pages.request("#api").is_none());
        assert!(!pages.stopped_short());
    }

    #[test]
    fn stops_at_the_start_of_history_or_the_limit() {
        let mut pages = Pages::new(Range {
            limit: Some(1500),
            ..Range::default()
        });
        assert_eq!(pages.request("#api").unwrap().params[3], "1000");
        let page: Vec<Seen> = (0..1000).map(|i| seen(&i.to_string(), "")).collect();
        pages.page(&page);
        assert_eq!(pages.request("#api").unwrap().params[3], "500");
        pages.page(&[]);
        assert!(pages.request("#api").is_none());
        assert_eq!(pages.excess(1700), 200);
    }

    #[test]
    fn says_when_it_stopped_short() {
        let mut pages = Pages::new(Range::default());
        for i in 0..MAX_PAGES {
            assert!(pages.request("#api").is_some());
            pages.page(&[seen(&i.to_string(), "")]);
        }
        assert!(pages.request("#api").is_none());
        assert!(pages.stopped_short());
    }
}
