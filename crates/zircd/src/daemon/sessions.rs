//! A session's full title and where its owner opens it ([`Request::Name`]): its card
//! ([`SessionCard`]) goes to the channels the session works in once its name, title or tool
//! is known and whenever one changes, so apps show its title first, its short handle under
//! it, and its tool beside its owner. Its title is the one it was given, else the one its
//! tool shows, as `zirc watch` reads it each time it starts, else its first task. The link
//! never leaves this machine: it's saved here and given only over zircd's socket
//! ([`Request::LocalSessions`]), to the owner's own apps.
//!
//! [`Request::Name`]: zirc_core::ipc::Request::Name
//! [`Request::LocalSessions`]: zirc_core::ipc::Request::LocalSessions

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use zirc_core::ipc::{LocalSession, Response};
use zirc_core::project::ORG_CHANNEL;
use zirc_proto::session::{self, SessionCard};

use super::Daemon;
use super::book::{Field, SEEN_EVERY, Shared};

/// The longest link kept, in bytes.
const MAX_LINK: usize = 2048;

/// What a session's card said of it: its name, if it was given one, its title and its tool.
pub(super) type Carded = (Option<String>, Option<String>, Option<String>);

/// What `book` says each session's card says, as a daemon starts: the cards it sent before
/// said so, and aren't sent again.
pub(super) fn carded(book: &Shared) -> BTreeMap<String, Carded> {
    book.sessions()
        .into_iter()
        .map(|session| {
            let said = carded_of(book, &session);
            (session, said)
        })
        .collect()
}

/// What `book` would have `session`'s card say, as [`Carded`].
fn carded_of(book: &Shared, session: &str) -> Carded {
    (
        book.get(Field::Name, session),
        book.title(session),
        book.get(Field::Tool, session),
    )
}

impl Daemon {
    /// Gives `session` its `title`, the title its tool shows (`tool_title`) and `link`, as
    /// said, and sends its card when its name, title or tool is no longer what its card last
    /// said here: the book is every organisation's daemon's, and each sends the card to its
    /// own channels.
    pub(super) fn title_session(
        &mut self,
        session: &str,
        (title, tool_title): (Option<&str>, Option<&str>),
        link: Option<&str>,
    ) -> Result<()> {
        if let Some(link) = link {
            let link = link.trim();
            ensure!(
                !link.is_empty() && link.len() <= MAX_LINK && !link.contains(char::is_whitespace),
                "--link is one address, without spaces, of at most {MAX_LINK} bytes"
            );
            self.book
                .set(Field::Link, session, link.to_owned())
                .context("couldn't save the session's link")?;
        }
        if let Some(title) = title {
            let title = session::title(title).context("--title has nothing to show")?;
            self.book
                .set(Field::Title, session, title)
                .context("couldn't save the session's title")?;
        }
        // As its tool shows it: read again each time a watch starts, so it keeps in step.
        if let Some(title) = tool_title.and_then(session::title) {
            self.book
                .set(Field::ToolTitle, session, title)
                .context("couldn't save the session's title")?;
        }
        self.send_card_if_changed(session);
        Ok(())
    }

    /// Keeps `task` as `session`'s first, its card's title where it has no other, when it
    /// has none yet.
    pub(super) fn first_task(&mut self, session: &str, task: &str) {
        if self.book.get(Field::FirstTask, session).is_some() {
            return;
        }
        let Some(task) = session::title(task) else {
            return;
        };
        if let Err(err) = self.book.set(Field::FirstTask, session, task) {
            tracing::warn!("couldn't save {session}'s first task: {err}");
        }
        self.send_card_if_changed(session);
    }

    /// Sends `session`'s card when what it says isn't what its card last said here.
    fn send_card_if_changed(&mut self, session: &str) {
        let says = carded_of(&self.book, session);
        if self.carded.get(session) != Some(&says) && says != (None, None, None) {
            self.carded.insert(session.to_owned(), says);
            self.send_card(session);
        }
    }

    /// Forgets what's kept of the sessions the book has forgotten, not seen for a week:
    /// their tasks, and what they were last told and said. Looked at every [`SEEN_EVERY`].
    pub(super) fn forget_sessions(&mut self, now: u64) {
        if now.saturating_sub(self.forgot_at) < SEEN_EVERY {
            return;
        }
        self.forgot_at = now;
        self.book.forget_old(now);
        let book = self.book.clone();
        self.forget_tasks();
        self.carded.retain(|session, _| book.knows(session));
        self.last_posts.retain(|session, _| book.knows(session));
        self.later_told.retain(|session, _| book.knows(session));
        let told = self.gotchas_told.len();
        self.gotchas_told.retain(|(session, _)| book.knows(session));
        if self.gotchas_told.len() < told {
            self.save_gotchas_told();
        }
    }

    /// Sends `session`'s card to the channels of the repositories it
    /// works in, or the organisation's channel when it works in none.
    fn send_card(&mut self, session: &str) {
        let Some(card) = self.card_of(session) else {
            return;
        };
        // The tenant's recorder hears it too, for zirc.dev's Working now (zirc_proto::working).
        self.record(session, card.event());
        let mut channels: BTreeSet<String> = self
            .repos
            .values()
            .filter(|watched| watched.sessions.contains(session))
            .map(|watched| watched.scope.clone())
            .collect();
        if channels.is_empty() {
            channels.insert(ORG_CHANNEL.to_owned());
        }
        for channel in channels {
            self.publish(&channel, session, card.event());
        }
    }

    /// `session`'s card, once its name, title or tool is known.
    pub(super) fn card_of(&self, session: &str) -> Option<SessionCard> {
        let card = SessionCard {
            session: session.to_owned(),
            name: self.name_of(session),
            title: self.book.title(session),
            tool: self.book.get(Field::Tool, session),
        };
        (card.name.is_some() || card.title.is_some() || card.tool.is_some()).then_some(card)
    }

    /// [`zirc_core::ipc::Request::LocalSessions`]: each session named, titled or linked here.
    pub(super) fn local_sessions(&self) -> Response {
        let named = self.book.sessions();
        let ids: BTreeSet<&str> = named
            .iter()
            .map(String::as_str)
            .chain(self.sessions_here())
            .collect();
        Response::LocalSessions {
            sessions: ids
                .into_iter()
                .map(|id| LocalSession {
                    session: id.to_owned(),
                    name: self.name_of(id),
                    title: self.book.title(id),
                    link: self.book.get(Field::Link, id),
                })
                .collect(),
        }
    }
}
