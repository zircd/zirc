//! Opening a conversation at one message, as a conversation's link or a search's result asks
//! ([`zirc_proto::link`]). Loaded already, it's [`Chat::found`] at once. Older, with
//! [`Options::around`], the messages around it come in one request (`CHATHISTORY AROUND`),
//! a stretch of their own: what's between them and what's newest is a hole
//! ([`Chat::hole`]), which the app fills a page at a time as the person scrolls down to it
//! ([`Chat::newer`]), while what's newest stays loaded below it. Without, its history pages
//! back until the message is there, as far as a conversation keeps
//! ([`Options::max_entries`]). Either way, once it's loaded the app brings it into view and
//! marks it; one the server no longer has is never found, and the conversation shows at its
//! end.
//!
//! [`Options::around`]: super::Options::around
//! [`Options::max_entries`]: super::Options::max_entries

use zirc_line::Line;

use super::{Ask, Chat, Page, fold};

/// What the hole below a stretch opened at a message says, as every app draws it there.
pub const HOLE_LABEL: &str = "Newer messages";

/// The control that goes back from a stretch opened at a message to what's newest.
pub const LATEST_LABEL: &str = "Jump to latest";

/// The message a conversation is opened at, while it's looked for.
#[derive(Debug, Clone)]
pub(super) struct Seek {
    /// The conversation, as it was opened.
    pub(super) name: String,
    /// The message's server id.
    pub(super) id: String,
}

impl Chat {
    /// Shows the conversation called `name`, as [`Chat::open`] does, at the message the
    /// server knows as `id`: once it's loaded, [`Chat::found`] says so. Returns what to send.
    pub fn open_at(&mut self, name: &str, id: &str) -> Vec<Line> {
        let mut lines = self.open(name);
        lines.extend(self.look_for(name, id));
        lines
    }

    /// Looks for the message `id` in the conversation called `name`, shown already or on
    /// its way, as [`Chat::open_at`] does. Returns what to send.
    pub fn look_for(&mut self, name: &str, id: &str) -> Option<Line> {
        self.seeking = Some(Seek {
            name: name.to_owned(),
            id: id.to_owned(),
        });
        self.seek(name)
    }

    /// The message [`Chat::open_at`] looked for, once it's loaded: its id, once, for the
    /// app to scroll to and mark.
    pub fn found(&mut self) -> Option<String> {
        self.found.take()
    }

    /// The id of the message in the conversation called `name` below which newer messages
    /// are missing, for the app to draw the hole there and ask for them ([`Chat::newer`])
    /// as the person reaches it. `None` when what it holds runs on to what's newest.
    #[must_use]
    pub fn hole(&self, name: &str) -> Option<&str> {
        self.conversation(name)?.hole.as_deref()
    }

    /// Asks for the page of messages after the hole in the conversation called `name`
    /// ([`Chat::hole`]), as the person scrolls down to it: `None` without a hole, or while
    /// a page of its history is on its way.
    pub fn newer(&mut self, name: &str) -> Option<Line> {
        let conversation = self.conversation(name)?;
        let after = conversation.hole.clone()?;
        let target = super::target(&conversation.name).to_owned();
        let folded = fold(&target);
        if self.gaps.contains_key(&folded) || !self.link.as_ref().is_some_and(|l| l.chathistory) {
            return None;
        }
        let count = self.options.history;
        Some(self.ask_page("AFTER", &target, &after, count, Ask::Newer))
    }

    /// Looks for the message being looked for in `target`, whose history just came: found,
    /// it's [`Chat::found`]; else it's asked for, once the history asked already is in.
    pub(super) fn seek(&mut self, target: &str) -> Option<Line> {
        let seek = self
            .seeking
            .as_ref()
            .filter(|seek| fold(&seek.name) == fold(target))?;
        let (name, id) = (seek.name.clone(), seek.id.clone());
        let conversation = self.conversation(&name);
        if conversation.is_some_and(|c| c.has_entry(&id)) {
            self.found = self.seeking.take().map(|seek| seek.id);
            return None;
        }
        // Not joined yet, or its history is still coming: it's looked for again then.
        if !conversation.is_some_and(|c| c.joined) || self.gaps.contains_key(&fold(target)) {
            return None;
        }
        if !self.link.as_ref().is_some_and(|link| link.chathistory) {
            self.seeking = None;
            return None;
        }
        if self.options.around {
            let target = super::target(&self.conversation(&name)?.name).to_owned();
            let count = self.options.history;
            return Some(self.ask_page("AROUND", &target, &id, count, Ask::Around(id.clone())));
        }
        // Not in what it keeps: it shows at its end.
        if !self.has_older(&name) {
            self.seeking = None;
            return None;
        }
        self.older(&name)
    }

    /// Whether any of `target`'s conversations passes `test`.
    fn any_of(&self, target: &str, test: impl Fn(&super::Conversation) -> bool) -> bool {
        self.conversations
            .iter()
            .filter(|c| super::target(&c.name).eq_ignore_ascii_case(target))
            .any(test)
    }

    /// The messages around `id` came, as [`Chat::seek`] asked: unless they met what's held,
    /// what's after them is a hole. Found, the app is told; not, the server doesn't have it.
    pub(super) fn around(&mut self, page: &Page, id: &str) {
        if !page.known
            && let Some(newest) = &page.newest
            && self.any_of(&page.target, |c| c.holds_after(newest))
        {
            self.set_hole(&page.target, Some(newest));
        }
        let held = self.any_of(&page.target, |c| c.has_entry(id));
        match self.seeking.take() {
            Some(seek) if seek.id == id && held => self.found = Some(seek.id),
            // Another looked for since: it's still looked for.
            Some(seek) if seek.id != id => self.seeking = Some(seek),
            _ => {}
        }
    }

    /// The hole in `target`'s conversations is below the message `after` now, or closed.
    pub(super) fn set_hole(&mut self, target: &str, after: Option<&str>) {
        for conversation in &mut self.conversations {
            if super::target(&conversation.name).eq_ignore_ascii_case(target)
                && conversation.hole.as_deref() != after
            {
                conversation.hole = after.map(str::to_owned);
                conversation.revision += 1;
            }
        }
    }

    /// A request for history failed (`FAIL CHATHISTORY`), naming its channel or not: a
    /// message asked around that the server doesn't have, so the conversation shows at its
    /// end. Says whether it was such a request.
    pub(super) fn history_failed(&mut self, line: &Line) -> bool {
        let target = line
            .params
            .iter()
            .map(String::as_str)
            .find(|param| super::is_channel(param));
        let around = self
            .gaps
            .iter()
            .find(|(folded, gap)| {
                matches!(gap.ask, Ask::Around(_)) && target.is_none_or(|t| fold(t) == **folded)
            })
            .map(|(folded, _)| folded.clone());
        let Some(folded) = around else {
            return false;
        };
        self.gaps.remove(&folded);
        self.seeking = None;
        true
    }
}
