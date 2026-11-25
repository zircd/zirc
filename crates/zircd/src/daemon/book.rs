//! What this machine's sessions go by ([`Request::Name`]): each one's name, the tool it runs
//! in, its picture, its full title and where its owner opens it, kept across restarts.
//!
//! One book for the machine, which every organisation's daemon reads and writes
//! ([`Shared`]): a session is the machine's, whichever organisation's repository it works
//! in, and the files are in zirc's own directory, not an organisation's. Each daemon once
//! kept its own copy and saved it over the others'.
//!
//! A session not seen for [`KEPT_FOR`] is forgotten: every agent session has a new id, and
//! the book once only grew.
//!
//! [`Request::Name`]: zirc_core::ipc::Request::Name

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use zirc_core::paths;

use super::{read_saved, save_saved};

/// How long after it was last seen a session is kept, in seconds: 7 days.
const KEPT_FOR: u64 = 7 * 86_400;

/// How often at most a session's being seen is saved, and each daemon looks for sessions to
/// forget, in seconds.
pub(super) const SEEN_EVERY: u64 = 3600;

/// One thing the book says of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Field {
    /// Its short name, which apps show in place of its id.
    Name,
    /// The agent tool it runs in.
    Tool,
    /// The picture it goes by.
    Avatar,
    /// Its full title as given (`zirc name --title`), which its card shows first.
    Title,
    /// Its title as its agent tool shows it, as `zirc watch` last read it.
    ToolTitle,
    /// The first task it took on (`zirc intend`).
    FirstTask,
    /// Where its owner opens it: never sent to the server.
    Link,
}

impl Field {
    const ALL: [Self; 7] = [
        Self::Name,
        Self::Tool,
        Self::Avatar,
        Self::Title,
        Self::ToolTitle,
        Self::FirstTask,
        Self::Link,
    ];

    /// Where it's saved, in zirc's directory `home`.
    fn path(self, home: &Path) -> PathBuf {
        match self {
            Self::Name => paths::session_names(home),
            Self::Tool => paths::session_tools(home),
            Self::Avatar => paths::session_avatars(home),
            Self::Title => paths::session_titles(home),
            Self::ToolTitle => paths::session_tool_titles(home),
            Self::FirstTask => paths::session_first_tasks(home),
            Self::Link => paths::session_links(home),
        }
    }
}

/// The book itself: what each field says, by session, and when each session was last seen.
#[derive(Debug, Default)]
struct Book {
    home: PathBuf,
    said: BTreeMap<Field, BTreeMap<String, String>>,
    /// Session → when it was last seen, in seconds since the Unix epoch.
    seen: BTreeMap<String, u64>,
}

impl Book {
    fn load(home: &Path, now: u64) -> Self {
        let said: BTreeMap<Field, BTreeMap<String, String>> = Field::ALL
            .into_iter()
            .map(|field| {
                let saved = read_saved(&field.path(home)).unwrap_or_default();
                (field, saved)
            })
            .collect();
        let mut seen: BTreeMap<String, u64> =
            read_saved(&paths::session_seen(home)).unwrap_or_default();
        // One in the book from before it kept when: seen now, so it has its whole time.
        let known = seen.len();
        for session in said.values().flat_map(BTreeMap::keys) {
            seen.entry(session.clone()).or_insert(now);
        }
        // Saved, or each restart would see them anew and they'd never be forgotten.
        if seen.len() > known
            && let Err(err) = save_saved(&paths::session_seen(home), &seen)
        {
            tracing::warn!("couldn't save when sessions were seen: {err}");
        }
        Self {
            home: home.to_owned(),
            said,
            seen,
        }
    }

    fn save(&self, field: Field) -> std::io::Result<()> {
        let empty = BTreeMap::new();
        save_saved(
            &field.path(&self.home),
            self.said.get(&field).unwrap_or(&empty),
        )
    }

    /// Forgets the sessions last seen before `now - KEPT_FOR`, and says which.
    fn forget_old(&mut self, now: u64) -> BTreeSet<String> {
        let old: BTreeSet<String> = self
            .seen
            .iter()
            .filter(|(_, at)| now.saturating_sub(**at) > KEPT_FOR)
            .map(|(session, _)| session.clone())
            .collect();
        if old.is_empty() {
            return old;
        }
        self.seen.retain(|session, _| !old.contains(session));
        for field in Field::ALL {
            let Some(said) = self.said.get_mut(&field) else {
                continue;
            };
            let before = said.len();
            said.retain(|session, _| !old.contains(session));
            if said.len() < before
                && let Err(err) = self.save(field)
            {
                tracing::warn!("couldn't save the sessions' {field:?}s: {err}");
            }
        }
        old
    }
}

/// The machine's one book, as each organisation's daemon holds it.
#[derive(Debug, Clone, Default)]
pub struct Shared(Arc<Mutex<Book>>);

impl Shared {
    /// The book saved in zirc's directory `home`, read at `now`.
    pub fn load(home: &Path, now: u64) -> Self {
        Self(Arc::new(Mutex::new(Book::load(home, now))))
    }

    /// The daemons take turns on one task, so nothing waits here; a daemon that panicked
    /// holding it left it as it was.
    fn book(&self) -> MutexGuard<'_, Book> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What `field` says of `session`.
    pub(super) fn get(&self, field: Field, session: &str) -> Option<String> {
        self.book().said.get(&field)?.get(session).cloned()
    }

    /// Has `field` say `value` of `session`, saved. Nothing to do when it says so already.
    pub(super) fn set(&self, field: Field, session: &str, value: String) -> std::io::Result<()> {
        let mut book = self.book();
        let said = book.said.entry(field).or_default();
        if said.get(session) == Some(&value) {
            return Ok(());
        }
        said.insert(session.to_owned(), value);
        book.save(field)
    }

    /// Every session given a name, with that name.
    pub(super) fn named(&self) -> Vec<(String, String)> {
        let book = self.book();
        book.said
            .get(&Field::Name)
            .into_iter()
            .flatten()
            .map(|(session, name)| (session.clone(), name.clone()))
            .collect()
    }

    /// The title `session`'s card shows: the one it was given, else the one its agent tool
    /// shows, else its first task (the user's choice, 25 Nov 2026).
    pub(super) fn title(&self, session: &str) -> Option<String> {
        [Field::Title, Field::ToolTitle, Field::FirstTask]
            .into_iter()
            .find_map(|field| self.get(field, session))
    }

    /// Every session the book names, titles, links or knows the tool of.
    pub(super) fn sessions(&self) -> BTreeSet<String> {
        let book = self.book();
        [
            Field::Name,
            Field::Tool,
            Field::Title,
            Field::ToolTitle,
            Field::FirstTask,
            Field::Link,
        ]
        .iter()
        .filter_map(|field| book.said.get(field))
        .flat_map(BTreeMap::keys)
        .cloned()
        .collect()
    }

    /// Whether `session` was seen in the last [`KEPT_FOR`].
    pub(super) fn knows(&self, session: &str) -> bool {
        self.book().seen.contains_key(session)
    }

    /// `session` was seen at `now`, doing something or being named: saved at most every
    /// [`SEEN_EVERY`].
    pub(super) fn seen(&self, session: &str, now: u64) {
        let mut book = self.book();
        let last = book.seen.get(session).copied();
        if last.is_some_and(|last| now.saturating_sub(last) < SEEN_EVERY) {
            return;
        }
        book.seen.insert(session.to_owned(), now);
        if let Err(err) = save_saved(&paths::session_seen(&book.home), &book.seen) {
            tracing::warn!("couldn't save when sessions were seen: {err}");
        }
    }

    /// Forgets the sessions not seen for [`KEPT_FOR`]. Each daemon then forgets what it keeps
    /// of the sessions the book no longer [`Shared::knows`].
    pub(super) fn forget_old(&self, now: u64) {
        let mut book = self.book();
        let old = book.forget_old(now);
        if !old.is_empty() {
            tracing::info!(
                "forgot {} sessions not seen for {} days",
                old.len(),
                KEPT_FOR / 86_400
            );
            if let Err(err) = save_saved(&paths::session_seen(&book.home), &book.seen) {
                tracing::warn!("couldn't save when sessions were seen: {err}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_daemon_reads_what_any_wrote_and_a_restart_keeps_it() {
        let home = tempfile::tempdir().unwrap();
        let book = Shared::load(home.path(), 100);
        let other = book.clone();
        book.set(Field::Name, "cc-1", "login".into()).unwrap();
        other.set(Field::Avatar, "cc-1", "rocket".into()).unwrap();
        other.set(Field::Title, "cc-2", "The API".into()).unwrap();
        assert_eq!(other.get(Field::Name, "cc-1").as_deref(), Some("login"));
        assert_eq!(book.get(Field::Avatar, "cc-1").as_deref(), Some("rocket"));
        assert_eq!(book.get(Field::Tool, "cc-1"), None);
        assert_eq!(
            book.sessions(),
            BTreeSet::from(["cc-1".to_owned(), "cc-2".to_owned()])
        );
        let again = Shared::load(home.path(), 200);
        assert_eq!(again.get(Field::Name, "cc-1").as_deref(), Some("login"));
        assert_eq!(again.get(Field::Title, "cc-2").as_deref(), Some("The API"));
    }

    #[test]
    fn a_session_not_seen_for_a_week_is_forgotten_and_one_seen_is_kept() {
        let home = tempfile::tempdir().unwrap();
        let start = 1_000_000;
        let book = Shared::load(home.path(), start);
        for session in ["cc-old", "cc-new"] {
            book.set(Field::Name, session, "x".into()).unwrap();
            book.set(Field::Link, session, "claude://x".into()).unwrap();
            book.seen(session, start);
        }
        let later = start + KEPT_FOR - 10;
        book.seen("cc-new", later);
        book.forget_old(later);
        assert!(book.knows("cc-old"), "not yet a week");
        book.forget_old(later + SEEN_EVERY);
        assert!(!book.knows("cc-old"));
        assert_eq!(book.get(Field::Name, "cc-old"), None);
        assert_eq!(book.get(Field::Link, "cc-old"), None);
        assert_eq!(book.get(Field::Name, "cc-new").as_deref(), Some("x"));
        // Forgotten on disk too, and what's kept is still there.
        let again = Shared::load(home.path(), later + SEEN_EVERY);
        assert_eq!(again.get(Field::Name, "cc-old"), None);
        assert!(again.knows("cc-new"));
    }

    #[test]
    fn a_session_named_before_the_book_kept_when_has_its_whole_time() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            paths::session_names(home.path()),
            r#"{"cc-before": "login"}"#,
        )
        .unwrap();
        let book = Shared::load(home.path(), 5_000_000);
        assert!(book.knows("cc-before"));
        book.forget_old(5_000_000 + KEPT_FOR - 1);
        assert_eq!(book.get(Field::Name, "cc-before").as_deref(), Some("login"));
    }
}
