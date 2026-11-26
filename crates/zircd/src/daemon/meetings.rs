//! Two sessions that keep meeting: their lines met in a third file, or they settled a second
//! time, within a few hours. Settling who lands first, a file at a time, sorts out each
//! conflict and never asks whether both should carry on as they are: on 24 Nov 2026 one pair
//! settled six times in 80 minutes. zircd tells each of the two, once, to ask the other what's
//! left of its task and agree who continues: one leaves files to the other, or builds on the
//! other's branch. One taking the other's task over changes what their user asked for, so
//! that needs the user's yes, as a request that waits for them (the user's choices, 25 Nov
//! 2026: prompted "when a pair keeps meeting", and "all but a takeover" theirs to agree).
//!
//! Counted for this machine's own sessions, whose lines it compares; kept only while zircd
//! runs.

use std::collections::{BTreeMap, BTreeSet};

use zirc_core::presence::{LocalOverlap, Overlap};
use zirc_proto::woke::Settled;

use super::{Daemon, now};

/// How many files two sessions' lines meet in before they're told.
const FILES: usize = 3;
/// How many times they settle before they're told.
const SETTLES: usize = 2;
/// How long a meeting counts for, and how long the two aren't told again, in seconds: as
/// long as a settle is remembered.
const WITHIN: u64 = zirc_core::clashes::SETTLED_FOR;

/// What two sessions' meetings have come to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Kept {
    /// Their lines have met in these files.
    Files(Vec<String>),
    /// They've settled this many times.
    Settles(usize),
}

/// What a pair has met in lately.
#[derive(Debug, Default)]
struct Met {
    /// The files their lines met in, each with when it last did.
    files: BTreeMap<String, u64>,
    /// When they settled.
    settles: Vec<u64>,
    /// When they were told.
    told: Option<u64>,
}

/// Each pair's meetings, by the repository's scope and the two sessions, in order.
#[derive(Debug, Default)]
pub(super) struct Meetings(BTreeMap<(String, String, String), Met>);

impl Meetings {
    fn of(&mut self, scope: &str, (a, b): (&str, &str), at: u64) -> &mut Met {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let met = self
            .0
            .entry((scope.to_owned(), a.to_owned(), b.to_owned()))
            .or_default();
        met.files
            .retain(|_, then| at.saturating_sub(*then) < WITHIN);
        met.settles.retain(|then| at.saturating_sub(*then) < WITHIN);
        met
    }

    /// The two met in the same lines of `files` at `at`: what that comes to, when it's now
    /// enough to tell them and they weren't told lately.
    fn met(&mut self, scope: &str, pair: (&str, &str), files: &[String], at: u64) -> Option<Kept> {
        let met = self.of(scope, pair, at);
        for file in files {
            met.files.insert(file.clone(), at);
        }
        let enough = met.files.len() >= FILES;
        Self::tell(met, enough, at).then(|| Kept::Files(met.files.keys().cloned().collect()))
    }

    /// The two settled at `at`: the same.
    fn settled(&mut self, scope: &str, pair: (&str, &str), at: u64) -> Option<Kept> {
        let met = self.of(scope, pair, at);
        met.settles.push(at);
        let enough = met.settles.len() >= SETTLES;
        Self::tell(met, enough, at).then_some(Kept::Settles(met.settles.len()))
    }

    /// Whether to tell them now, noting that they were.
    fn tell(met: &mut Met, enough: bool, at: u64) -> bool {
        let lately = met
            .told
            .is_some_and(|then| at.saturating_sub(then) < WITHIN);
        if enough && !lately {
            met.told = Some(at);
        }
        enough && !lately
    }
}

/// What `session` hears when it and `other` keep meeting.
fn keeps_meeting_line(session: &str, other: &str, kept: &Kept) -> String {
    let how = match kept {
        Kept::Files(files) => {
            let named: Vec<String> = files.iter().map(|file| format!("`{file}`")).collect();
            format!(
                "your lines have met in {} files ({})",
                files.len(),
                zirc_proto::some_of(&named, 4, ", ")
            )
        }
        Kept::Settles(times) => format!("you've settled {times} times"),
    };
    format!(
        "{session}: you and {other} keep meeting: {how}. Before either of you carries on, ask \
         it what's left of its task and agree who continues, not only who lands first: `zirc \
         ask --session {session} --to {other}`. One of you can leave those files to the other, \
         or build on the other's branch; say which with `zirc settle --with {other} --leaves \
         <files>` or `--builds-on <files>`, and tell your user in a line. If one of you should take the other's whole task over, \
         that's your users' call: ask with `zirc request --session {session}`, which waits for \
         their yes"
    )
}

impl Daemon {
    /// The two sessions of `overlap`, one between this machine's worktrees, met in its
    /// same lines: each is told when they keep meeting.
    pub(super) fn met_in(&mut self, channel: &str, overlap: &LocalOverlap) {
        if overlap.same_lines.is_empty() {
            return;
        }
        let sessions: BTreeSet<String> = overlap
            .worktrees
            .iter()
            .filter_map(|worktree| self.session_of(worktree).map(str::to_owned))
            .collect();
        let [a, b] = &sessions.into_iter().collect::<Vec<_>>()[..] else {
            return;
        };
        if self.settled_here(channel, a, (None, b)) {
            return;
        }
        let files: Vec<String> = overlap.same_lines.keys().cloned().collect();
        if let Some(kept) = self.meetings.met(channel, (a, b), &files, now()) {
            self.tell_keeps_meeting(channel, (a, b), &kept);
        }
    }

    /// The session of `overlap`'s worktree here met another machine's sessions in its same
    /// lines: it's told when it keeps meeting one, as that one is by its own machine.
    pub(super) fn met_remotely(&mut self, overlap: &Overlap) {
        let Some(mine) = overlap
            .worktree
            .as_ref()
            .and_then(|worktree| self.session_of(worktree))
            .map(str::to_owned)
        else {
            return;
        };
        let files: Vec<String> = overlap.same_lines.keys().cloned().collect();
        if files.is_empty() {
            return;
        }
        for other in &overlap.sessions {
            if self.settled_here(&overlap.channel, &mine, (Some(&overlap.nick), other)) {
                continue;
            }
            let theirs = format!("{}/{other}", overlap.nick);
            if let Some(kept) = self
                .meetings
                .met(&overlap.channel, (&mine, &theirs), &files, now())
            {
                tracing::info!(
                    "{mine} and {theirs} keep meeting in {}: {kept:?}",
                    overlap.channel
                );
                self.tell_dropped(&overlap.channel, keeps_meeting_line(&mine, &theirs, &kept));
            }
        }
    }

    /// Whether `mine`, a session here, and `other`, on `machine` or here, have a settle
    /// standing in `scope` ([`Daemon::settled_with`]): their meeting elsewhere meanwhile is
    /// what the settle already covers, so it doesn't count towards telling them they keep
    /// meeting. A session that had just settled one file was told so of four at its next
    /// wake (25 Nov 2026); settling a second time still tells them.
    fn settled_here(
        &self,
        scope: &str,
        mine: &str,
        (machine, other): (Option<&str>, &str),
    ) -> bool {
        let Some(me) = self.client.as_ref().map(|client| client.nick().to_owned()) else {
            return false;
        };
        self.settled_with(scope, (&me, mine), (machine.unwrap_or(&me), other))
    }

    /// `settled`, said in `scope`, is between two of this machine's sessions: each is told
    /// when they keep meeting.
    pub(super) fn settled_again(&mut self, scope: &str, settled: &Settled) {
        let Some(me) = self.client.as_ref().map(|client| client.nick().to_owned()) else {
            return;
        };
        let (first, second) = (&settled.first, &settled.second);
        if !first.machine.eq_ignore_ascii_case(&me) || !second.machine.eq_ignore_ascii_case(&me) {
            return;
        }
        let pair = (first.session.as_str(), second.session.as_str());
        if let Some(kept) = self.meetings.settled(scope, pair, now()) {
            self.tell_keeps_meeting(scope, pair, &kept);
        }
    }

    fn tell_keeps_meeting(&mut self, channel: &str, (a, b): (&str, &str), kept: &Kept) {
        tracing::info!("{a} and {b} keep meeting in {channel}: {kept:?}");
        for (session, other) in [(a, b), (b, a)] {
            self.tell_dropped(channel, keeps_meeting_line(session, other, kept));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(names: &[&str]) -> Vec<String> {
        names.iter().map(|&name| name.to_owned()).collect()
    }

    #[test]
    fn a_third_file_tells_the_pair_once_and_not_again_for_a_while() {
        let mut meetings = Meetings::default();
        let pair = ("cc-2", "cc-1");
        assert_eq!(
            meetings.met("#api", pair, &files(&["a.rs", "b.rs"]), 100),
            None
        );
        // The same file again is no third; another repository's or another pair's neither.
        assert_eq!(meetings.met("#api", pair, &files(&["a.rs"]), 200), None);
        assert_eq!(meetings.met("#web", pair, &files(&["c.rs"]), 200), None);
        assert_eq!(
            meetings.met("#api", ("cc-1", "cc-3"), &files(&["c.rs"]), 200),
            None
        );
        // Whichever way round the two are named.
        assert_eq!(
            meetings.met("#api", ("cc-1", "cc-2"), &files(&["c.rs"]), 300),
            Some(Kept::Files(files(&["a.rs", "b.rs", "c.rs"])))
        );
        assert_eq!(meetings.met("#api", pair, &files(&["d.rs"]), 400), None);
        // Long after, with three files met lately again, they're told again.
        let later = 400 + WITHIN;
        assert_eq!(
            meetings.met("#api", pair, &files(&["e.rs", "f.rs"]), later),
            None
        );
        assert!(
            meetings
                .met("#api", pair, &files(&["g.rs"]), later + 1)
                .is_some()
        );
    }

    #[test]
    fn a_second_settle_tells_them_and_one_long_before_doesnt_count() {
        let mut meetings = Meetings::default();
        let pair = ("cc-1", "cc-2");
        assert_eq!(meetings.settled("#api", pair, 100), None);
        assert_eq!(meetings.settled("#api", pair, 100 + WITHIN), None);
        assert_eq!(
            meetings.settled("#api", pair, 200 + WITHIN),
            Some(Kept::Settles(2))
        );
        assert_eq!(meetings.settled("#api", pair, 300 + WITHIN), None);
    }

    #[test]
    fn the_line_names_the_other_the_files_and_what_they_may_agree() {
        let line = keeps_meeting_line(
            "cc-1",
            "cc-2",
            &Kept::Files(files(&["a.rs", "b.rs", "c.rs"])),
        );
        assert!(
            line.starts_with(
                "cc-1: you and cc-2 keep meeting: your lines have met in 3 files (`a.rs`, `b.rs`, `c.rs`)."
            ),
            "{line}"
        );
        assert!(
            line.contains("`zirc settle --with cc-2 --leaves <files>`"),
            "{line}"
        );
        assert!(
            line.contains("`zirc ask --session cc-1 --to cc-2`"),
            "{line}"
        );
        assert!(line.contains("`zirc request --session cc-1`"), "{line}");
        let line = keeps_meeting_line("cc-2", "cc-1", &Kept::Settles(2));
        assert!(line.contains("you've settled 2 times"), "{line}");
    }
}
