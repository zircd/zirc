//! The intents zircd declares for a session by itself: once the session changes files in a
//! directory, an intent for that directory goes to its repository's channel, renewed while
//! the changes go on, unless the session declared one of its own with `zirc intend`. In the
//! team's chat of 31 Oct to 4 Nov 2026, only a third of landings had an intent behind them,
//! yet intents gave warning of half of all conflicts: they name directories before a
//! file there is touched by anyone else.
//!
//! An automatic intent names directories, not files, and lasts [`LASTS`]: a new directory
//! is said at most once every [`GAP`], and the same ones again every [`RENEW`] while the
//! session still has changes there. A file at the repository's root is named itself, as
//! its directory would be the whole repository and touch everyone's work, and lockfiles
//! not at all: whoever lands second just writes theirs again.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use zirc_proto::Event;

use super::{Daemon, now};

/// How long an automatic intent lasts, in seconds.
pub(super) const LASTS: u64 = 30 * 60;

/// How often at most, in seconds, a session's automatic intent is said again for new
/// directories.
const GAP: u64 = 60;

/// After how long, in seconds, the same directories are said again while the session is
/// still changing them, before the last one expires.
const RENEW: u64 = 20 * 60;

/// What's due for a session's automatic intent.
#[derive(Debug, PartialEq, Eq)]
enum Due {
    /// Say it, naming these directories.
    Say(Vec<String>),
    /// Say it again to keep it going, naming the same directories or fewer: a renewal,
    /// which is no line of chat and wakes nobody ([`zirc_proto::renewal`]).
    Renew(Vec<String>),
    /// Withdraw the one said, for these directories: the session's changes are gone, as
    /// committed, restored or with their worktree removed.
    Withdraw(Vec<String>),
}

/// What's been said for each session, and until when each session's own intent lasts.
#[derive(Debug, Default)]
pub(super) struct AutoIntents {
    /// By session: the directories last said, and when.
    said: BTreeMap<String, (BTreeSet<String>, u64)>,
    /// By session: the intent it declared itself.
    own: BTreeMap<String, Own>,
}

/// An intent a session declared itself (`zirc intend`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Own {
    /// When it ends, in seconds since the Unix epoch.
    ends: u64,
    /// The channel it was said in.
    pub(super) channel: String,
    /// The paths it named.
    pub(super) paths: Vec<String>,
    /// Whether a landing the session queued since it was said (`//ops:land`) carries the
    /// changes it named: that landing landing ends it.
    landing: bool,
}

impl AutoIntents {
    /// Notes that `session` declared an intent of its own, naming `paths` in `channel` and
    /// ending `expires`: none is said for it automatically meanwhile. Its own replaced
    /// zircd's, so there's none of zircd's to withdraw: withdrawing it would end the
    /// session's. It belongs to no landing yet: one stated after a landing was queued is for
    /// what comes next.
    pub(super) fn declared(
        &mut self,
        session: &str,
        channel: &str,
        paths: &[String],
        expires: u64,
    ) {
        self.own.insert(
            session.to_owned(),
            Own {
                ends: expires,
                channel: channel.to_owned(),
                paths: paths.to_vec(),
                landing: false,
            },
        );
        self.said.remove(session);
    }

    /// Notes that `session` queued a landing at `now`: the intent it declared before, still
    /// going, is for what that landing carries.
    pub(super) fn landing_queued(&mut self, session: &str, now: u64) {
        if let Some(own) = self.own.get_mut(session).filter(|own| own.ends > now) {
            own.landing = true;
        }
    }

    /// `session`'s landing landed at `now`: the intent it declared before queuing it, if
    /// that's still going, which ends now, as its changes are no longer to come. None when it
    /// declared another since, for what's next.
    pub(super) fn landed(&mut self, session: &str, now: u64) -> Option<Own> {
        let ended = self
            .own
            .get(session)
            .is_some_and(|own| own.landing && own.ends > now);
        ended.then(|| self.own.remove(session)).flatten()
    }

    /// What's due for `session`, changing `paths` at `now`: an intent naming new
    /// directories or renewed, its withdrawal once its changes are gone, or nothing.
    fn due(&mut self, session: &str, paths: &BTreeSet<String>, now: u64) -> Option<Due> {
        if paths.is_empty() {
            return self.withdraw(session).map(Due::Withdraw);
        }
        if self.own.get(session).is_some_and(|own| own.ends > now) {
            return None;
        }
        let dirs: BTreeSet<String> = paths
            .iter()
            .filter(|path| !zirc_core::hunks::is_lockfile(path))
            .map(|path| directory(path))
            .collect();
        if dirs.is_empty() {
            return None;
        }
        let due = match self.said.get(session) {
            None => Some(false),
            Some((said, at)) => {
                let since = now.saturating_sub(*at);
                let more = !dirs.is_subset(said);
                if more && since >= GAP {
                    Some(false)
                } else if since >= RENEW {
                    Some(!more)
                } else {
                    None
                }
            }
        };
        let renews = due?;
        self.said.insert(session.to_owned(), (dirs.clone(), now));
        let dirs = dirs.into_iter().collect();
        Some(if renews {
            Due::Renew(dirs)
        } else {
            Due::Say(dirs)
        })
    }

    /// Forgets `session`'s automatic intent, and the directories it named, if one was said.
    fn withdraw(&mut self, session: &str) -> Option<Vec<String>> {
        self.said
            .remove(session)
            .map(|(dirs, _)| dirs.into_iter().collect())
    }
}

/// zircd's intent for `session` on `branch`, changing `files` in `dirs`, lasting `lasts`
/// seconds from `now`. Its words name the files, so whoever reads it in a list or an app
/// sees which, not just where; its paths stay the directories, to warn of what comes next
/// there.
fn auto_intent(
    branch: Option<&str>,
    files: &BTreeSet<String>,
    dirs: Vec<String>,
    now: u64,
    lasts: u64,
) -> Event {
    const SHOWN: usize = 3;
    let files: Vec<&str> = files
        .iter()
        .filter(|path| !zirc_core::hunks::is_lockfile(path))
        .map(String::as_str)
        .collect();
    let what = match files.len() {
        0 => "files here".to_owned(),
        n if n <= SHOWN => files.join(", "),
        n => format!("{} and {} more", files[..SHOWN].join(", "), n - SHOWN),
    };
    let summary = match branch {
        Some(branch) => format!("changing {what} on {branch} (said by zircd)"),
        None => format!("changing {what} (said by zircd)"),
    };
    Event::Intent {
        summary,
        paths: dirs,
        expires: Some(now + lasts),
        lasts: Some(lasts),
    }
}

/// The directory `path` is in, relative to the repository's root, or `path` itself for one
/// at the root.
fn directory(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((dir, _)) if !dir.is_empty() => dir.to_owned(),
        _ => path.to_owned(),
    }
}

impl Daemon {
    /// Says an intent for each session changing files in the repository at `root` that's
    /// due one (see the module's doc).
    pub(super) fn auto_intend(&mut self, root: &Path) {
        let Some(watched) = self.repos.get(root) else {
            return;
        };
        let channel = watched.scope.clone();
        let sessions: Vec<(String, BTreeSet<String>, Option<String>)> = watched
            .sessions
            .names()
            .map(|session| {
                let branch = watched
                    .sets()
                    .into_iter()
                    .find(|set| set.session == Some(session))
                    .and_then(|set| set.branch);
                (session.to_owned(), watched.paths_of(session), branch)
            })
            .collect();
        let now = now();
        for (session, paths, branch) in sessions {
            let (dirs, renews) = match self.auto_intents.due(&session, &paths, now) {
                Some(Due::Say(dirs)) => (dirs, false),
                Some(Due::Renew(dirs)) => (dirs, true),
                Some(Due::Withdraw(dirs)) => {
                    self.withdraw_intent(&channel, &session, dirs, now);
                    continue;
                }
                None => continue,
            };
            if !renews {
                self.tell_gotchas(&channel, &session, &dirs);
            }
            let intent = auto_intent(branch.as_deref(), &paths, dirs, now, LASTS);
            let event = if renews {
                zirc_proto::renewal(&intent).unwrap_or(intent)
            } else {
                intent
            };
            self.publish_debug(&channel, &session, event);
        }
    }

    /// Says again in `channel`, an agent room, the automatic intents still going there, for
    /// someone who just joined: they're debug, so no history holds them (as working sets).
    pub(super) fn say_auto_intents_again(&mut self, channel: &str) {
        let now = now();
        let again: Vec<(String, Event)> = self
            .repos
            .values()
            .filter(|watched| watched.scope == channel)
            .flat_map(|watched| {
                watched.sessions.names().filter_map(|session| {
                    let (dirs, at) = self.auto_intents.said.get(session)?;
                    let left = (at + LASTS).checked_sub(now).filter(|&left| left > 0)?;
                    let branch = watched
                        .sets()
                        .into_iter()
                        .find(|set| set.session == Some(session))
                        .and_then(|set| set.branch);
                    let dirs = dirs.iter().cloned().collect();
                    let files = watched.paths_of(session);
                    Some((
                        session.to_owned(),
                        auto_intent(branch.as_deref(), &files, dirs, now, left),
                    ))
                })
            })
            .collect();
        for (session, intent) in again {
            self.publish_debug(channel, &session, intent);
        }
    }

    /// Sends `event` from `session` to `channel` as debug: live, out of its history.
    fn publish_debug(&mut self, channel: &str, session: &str, event: Event) {
        let message = zirc_proto::Message {
            level: Some(zirc_proto::Level::Debug),
            ..self.signed(session, event)
        };
        self.publish_message(channel, message);
    }

    /// Ends the intent `session` declared before queuing a landing, now that it has landed,
    /// everywhere, as `done:` would ([`AutoIntents::landed`]): its changes are no longer to
    /// come, and sessions changing those paths would hear of them for nothing.
    pub(super) fn intent_landed(&mut self, session: &str) {
        let now = now();
        if let Some(own) = self.auto_intents.landed(session, now) {
            self.publish_debug(
                &own.channel,
                session,
                Event::Intent {
                    summary: "done: landed (said by zircd)".to_owned(),
                    paths: own.paths,
                    expires: Some(now),
                    lasts: Some(0),
                },
            );
        }
    }

    /// Withdraws in `channel`, the agent room they were said in, the automatic intents of
    /// `sessions`, as when their checkout is gone.
    pub(super) fn withdraw_auto_intents(&mut self, channel: &str, sessions: &[String]) {
        let now = now();
        for session in sessions {
            if let Some(dirs) = self.auto_intents.withdraw(session) {
                self.withdraw_intent(channel, session, dirs, now);
            }
        }
    }

    /// Says `session`'s automatic intent for `dirs` again, ending `now`, which ends it
    /// everywhere: an intent replaces the session's last, and one that's ended lists and
    /// wakes nobody.
    fn withdraw_intent(&mut self, channel: &str, session: &str, dirs: Vec<String>, now: u64) {
        self.publish_debug(
            channel,
            session,
            Event::Intent {
                summary: "done changing files here (said by zircd)".to_owned(),
                paths: dirs,
                expires: Some(now),
                lasts: Some(0),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|&path| path.to_owned()).collect()
    }

    #[test]
    fn says_new_directories_soon_and_the_same_ones_now_and_then() {
        let mut auto = AutoIntents::default();
        let first = paths(&["src/daemon.rs", "src/lib.rs", "README.md"]);
        assert_eq!(
            auto.due("cc-1", &first, 1000),
            Some(Due::Say(vec!["README.md".to_owned(), "src".to_owned()]))
        );
        // The same directories: not again until it's time to renew.
        assert_eq!(auto.due("cc-1", &paths(&["src/git.rs"]), 1100), None);
        // A new one, but within a minute of the last: it waits.
        let more = paths(&["src/daemon.rs", "tests/accounts.rs"]);
        assert_eq!(auto.due("cc-1", &more, 1030), None);
        assert_eq!(
            auto.due("cc-1", &more, 1060),
            Some(Due::Say(vec!["src".to_owned(), "tests".to_owned()]))
        );
        assert_eq!(auto.due("cc-1", &more, 1060 + RENEW - 1), None);
        // The same ones, or fewer, in time: renewed, which is no line of chat.
        assert_eq!(
            auto.due("cc-1", &more, 1060 + RENEW),
            Some(Due::Renew(vec!["src".to_owned(), "tests".to_owned()]))
        );
        let fewer = paths(&["src/daemon.rs"]);
        assert_eq!(
            auto.due("cc-1", &fewer, 1060 + 2 * RENEW),
            Some(Due::Renew(vec!["src".to_owned()]))
        );
        // A new one is said, not renewed, as soon as a minute allows.
        assert_eq!(
            auto.due("cc-1", &more, 1060 + 2 * RENEW + GAP),
            Some(Due::Say(vec!["src".to_owned(), "tests".to_owned()]))
        );
        // Its changes committed or restored: withdrawn at once, once, and said afresh next
        // time.
        assert_eq!(
            auto.due("cc-1", &BTreeSet::new(), 5000),
            Some(Due::Withdraw(vec!["src".to_owned(), "tests".to_owned()]))
        );
        assert_eq!(auto.due("cc-1", &BTreeSet::new(), 5001), None);
        assert!(auto.due("cc-1", &more, 5002).is_some());
    }

    #[test]
    fn names_root_files_themselves_and_leaves_lockfiles_out() {
        let mut auto = AutoIntents::default();
        let locks = paths(&["Cargo.lock", "MODULE.bazel.lock", "web/pnpm-lock.yaml"]);
        assert_eq!(auto.due("cc-1", &locks, 1000), None);
        let root = paths(&["Cargo.lock", "MODULE.bazel", "CLAUDE.md"]);
        assert_eq!(
            auto.due("cc-1", &root, 1000),
            Some(Due::Say(vec![
                "CLAUDE.md".to_owned(),
                "MODULE.bazel".to_owned()
            ]))
        );
    }

    #[test]
    fn says_nothing_while_the_session_has_an_intent_of_its_own() {
        let mut auto = AutoIntents::default();
        auto.declared("cc-1", "#a", &[], 2000);
        let changing = paths(&["src/daemon.rs"]);
        assert_eq!(auto.due("cc-1", &changing, 1999), None);
        assert!(auto.due("cc-2", &changing, 1999).is_some());
        assert_eq!(
            auto.due("cc-1", &changing, 2000),
            Some(Due::Say(vec!["src".to_owned()]))
        );
    }

    #[test]
    fn an_intent_of_the_sessions_own_isnt_withdrawn_for_it() {
        let mut auto = AutoIntents::default();
        let changing = paths(&["src/daemon.rs"]);
        assert!(auto.due("cc-1", &changing, 1000).is_some());
        // `zirc intend` replaced zircd's intent; then the session committed. Withdrawing
        // zircd's now would end the session's own.
        auto.declared("cc-1", "#a", &[], 3000);
        assert_eq!(auto.due("cc-1", &BTreeSet::new(), 1100), None);
        assert_eq!(auto.withdraw("cc-1"), None);
    }

    #[test]
    fn a_landing_ends_the_intent_said_before_it_was_queued_but_not_one_said_after() {
        let mut auto = AutoIntents::default();
        let named = vec!["src/daemon.rs".to_owned()];
        auto.declared("cc-1", "#a", &named, 5000);
        // Not queued: landing nothing ends it.
        assert_eq!(auto.landed("cc-1", 1000), None);
        auto.landing_queued("cc-1", 1100);
        let ended = auto.landed("cc-1", 1200).expect("ended by its landing");
        assert_eq!((ended.channel.as_str(), ended.paths), ("#a", named.clone()));
        assert_eq!(auto.landed("cc-1", 1300), None, "once");
        // Said after the landing was queued: for what's next, which that landing doesn't end.
        auto.declared("cc-1", "#a", &named, 5000);
        auto.landing_queued("cc-1", 1400);
        auto.declared("cc-1", "#a", &named, 6000);
        assert_eq!(auto.landed("cc-1", 1500), None);
        // Over by itself: nothing to end.
        auto.landing_queued("cc-1", 1600);
        assert_eq!(auto.landed("cc-1", 7000), None);
    }
}
