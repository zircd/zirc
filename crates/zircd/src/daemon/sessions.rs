//! A session's full title and where its owner opens it ([`Request::Name`]): the title goes
//! to the channels the session works in as a [`SessionCard`], when it's given and whenever
//! it or the session's name changes, so apps can show it beside the short handle. The link
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

/// What a session's card said of it: its name, if it was given one, and its title.
pub(super) type Carded = (Option<String>, Option<String>);

/// What `book` says each session's card says, as a daemon starts: the cards it sent before
/// said so, and aren't sent again.
pub(super) fn carded(book: &Shared) -> BTreeMap<String, Carded> {
    book.sessions()
        .into_iter()
        .map(|session| {
            let said = (
                book.get(Field::Name, &session),
                book.get(Field::Title, &session),
            );
            (session, said)
        })
        .collect()
}

impl Daemon {
    /// Gives `session` its `title` and `link`, as said, and sends its card when its name or
    /// its title is no longer what its card last said here: the book is every
    /// organisation's daemon's, and each sends the card to its own channels.
    pub(super) fn title_session(
        &mut self,
        session: &str,
        title: Option<&str>,
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
        let says = (
            self.book.get(Field::Name, session),
            self.book.get(Field::Title, session),
        );
        if self.carded.get(session) != Some(&says) && says != (None, None) {
            self.carded.insert(session.to_owned(), says);
            self.send_card(session);
        }
        Ok(())
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

    /// Sends `session`'s card, if it has a title, to the channels of the repositories it
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

    /// `session`'s card, if it has a title.
    pub(super) fn card_of(&self, session: &str) -> Option<SessionCard> {
        Some(SessionCard {
            session: session.to_owned(),
            name: self.name_of(session),
            title: self.book.get(Field::Title, session)?,
            tool: self.book.get(Field::Tool, session),
        })
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
                    title: self.book.get(Field::Title, id),
                    link: self.book.get(Field::Link, id),
                })
                .collect(),
        }
    }
}
