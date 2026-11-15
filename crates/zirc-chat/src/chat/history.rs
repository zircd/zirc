//! Asking the server for history, and taking it in.

use super::{
    Ask, Chat, DateTime, EntryKind, Event, Gap, Kind, Line, Page, Utc, fold, is_channel, is_service,
};

/// How long before the newest message held a catch-up asks from, so that message, and any
/// said in the same instant, comes back with what's new: a page that has one held already
/// is known to have met what's held.
const CATCH_UP_OVERLAP: chrono::Duration = chrono::Duration::seconds(1);

impl Chat {
    pub(super) fn next_batch(&mut self) -> String {
        self.sent_batches += 1;
        format!("{}{}", self.options.batch_prefix, self.sent_batches)
    }

    /// Asks for what's been said in `target` since the newest message we have of it, or
    /// for its latest history when we have none. Each page is [`Options::history`]
    /// messages, newest first; while one comes back full, the page before it follows.
    ///
    /// Since its time, a little before ([`CATCH_UP_OVERLAP`]), not its id: a server answers
    /// an id it no longer has (the message trimmed, past what it keeps, its history lost)
    /// with nothing, as it does when nothing was said, and what was said since would never
    /// show. From a time it answers whatever it has, and what's held already is known by
    /// its id.
    pub(super) fn catch_up(&mut self, target: &str) -> Line {
        // A machine's history holds what was said with each of its agents too.
        let newest = self
            .conversations
            .iter()
            .filter(|c| super::target(&c.name).eq_ignore_ascii_case(target))
            .filter_map(|c| c.entries.iter().rev().find(|entry| entry.id.is_some()))
            .max_by_key(|entry| entry.time)
            .and_then(|entry| Some((entry.id.clone()?, entry.time)));
        let Some((newest, said)) = newest else {
            return self.latest(target);
        };
        let line = Line::new(
            "CHATHISTORY",
            [
                "LATEST",
                target,
                &format!("timestamp={}", super::stamp(said - CATCH_UP_OVERLAP)),
                &self.options.history.to_string(),
            ],
        );
        self.gaps.insert(
            fold(target),
            Gap {
                since: Some(newest),
                pages: 1,
                ask: Ask::Catch,
            },
        );
        line
    }

    /// A page filling a gap ended: ask for the one before it, unless the gap's closed or
    /// it's been filled with as much as a conversation keeps.
    pub(super) fn filled(&mut self, page: Page) -> Option<Line> {
        let folded = fold(&page.target);
        let history = self.options.history;
        let enough = self.options.max_entries.div_ceil(history.max(1));
        let shown = self.shown(&page.target);
        let screenful = self.options.screenful;
        let gap = self.gaps.get(&folded)?;
        match gap.ask.clone() {
            Ask::Around(id) => {
                self.gaps.remove(&folded);
                self.around(&page, &id);
                return None;
            }
            Ask::Newer => {
                self.gaps.remove(&folded);
                // A short page reached what's newest; one with a message held met it.
                let closed = page.known || page.count < history;
                self.set_hole(&page.target, page.newest.as_deref().filter(|_| !closed));
                return None;
            }
            Ask::Catch | Ask::Older => {}
        }
        // A short page of the newest or of older history: there's none before it.
        if gap.since.is_none() && page.count < history {
            self.began(&page.target);
        }
        let gap = self.gaps.get_mut(&folded)?;
        if gap.ask == Ask::Older {
            self.gaps.remove(&folded);
            return None;
        }
        // A fresh conversation's goes on while there's too little to read.
        let wanted = gap.since.is_some() || shown < screenful;
        let oldest = page
            .oldest
            .filter(|_| wanted && page.count >= history && !page.reached && gap.pages < enough);
        let Some(oldest) = oldest else {
            self.gaps.remove(&folded);
            return None;
        };
        gap.pages += 1;
        Some(Line::new(
            "CHATHISTORY",
            [
                "BEFORE",
                &page.target,
                &format!("msgid={oldest}"),
                &history.to_string(),
            ],
        ))
    }

    /// Asks for the page of history before the oldest message the conversation called `name`
    /// holds, as the person scrolls to the top of what's loaded: `None` while a page of it
    /// is on its way, once it goes back to the start ([`super::Conversation::beginning`]) or
    /// holds as much as a conversation keeps, or without the server's history.
    pub fn older(&mut self, name: &str) -> Option<Line> {
        if !self.has_older(name) {
            return None;
        }
        let target = super::target(&self.conversation(name)?.name).to_owned();
        let folded = fold(&target);
        if self.gaps.contains_key(&folded) {
            return None;
        }
        // A machine's history holds what was said with each of its agents too.
        let oldest = self
            .conversations
            .iter()
            .filter(|c| super::target(&c.name).eq_ignore_ascii_case(&target))
            .filter_map(|c| c.entries.iter().find(|entry| entry.id.is_some()))
            .min_by_key(|entry| entry.time)
            .and_then(|entry| entry.id.clone())?;
        self.gaps.insert(
            folded,
            Gap {
                since: None,
                pages: 1,
                ask: Ask::Older,
            },
        );
        Some(Line::new(
            "CHATHISTORY",
            [
                "BEFORE",
                &target,
                &format!("msgid={oldest}"),
                &self.options.history.to_string(),
            ],
        ))
    }

    /// Whether a page of the history of the conversation called `name` is on its way: what
    /// it holds may not be all there is to show yet.
    #[must_use]
    pub fn history_coming(&self, name: &str) -> bool {
        self.gaps.contains_key(&fold(super::target(name)))
    }

    /// Whether the conversation called `name` may have older history than it holds, which
    /// [`Chat::older`] asks for: until a page comes back short, short of as much as a
    /// conversation keeps, with the server's history.
    #[must_use]
    pub fn has_older(&self, name: &str) -> bool {
        self.link.as_ref().is_some_and(|link| link.chathistory)
            && self.conversation(name).is_some_and(|conversation| {
                !conversation.beginning && conversation.entries.len() < self.options.max_entries
            })
    }

    /// No conversation's history goes back further than what it holds, so none asks for
    /// older: a showcase's, whose lines are all there are, with no server to answer.
    pub fn all_began(&mut self) {
        for conversation in &mut self.conversations {
            conversation.beginning = true;
        }
    }

    /// `target`'s history goes back no further than what its conversations hold.
    fn began(&mut self, target: &str) {
        for conversation in &mut self.conversations {
            if super::target(&conversation.name).eq_ignore_ascii_case(target) {
                conversation.beginning = true;
            }
        }
    }

    /// Asks for the latest history of `target`, and the pages before it while too little of
    /// it is what people said ([`Options::screenful`]).
    pub(super) fn latest(&mut self, target: &str) -> Line {
        self.gaps.insert(
            fold(target),
            Gap {
                since: None,
                pages: 1,
                ask: Ask::Catch,
            },
        );
        Line::new(
            "CHATHISTORY",
            ["LATEST", target, "*", &self.options.history.to_string()],
        )
    }

    /// How much of what's kept of `target`'s conversations is what people said: messages,
    /// actions, notices, and agents' posts and files, not machines' events or joins.
    pub(super) fn shown(&self, target: &str) -> usize {
        self.conversations
            .iter()
            .filter(|c| super::target(&c.name).eq_ignore_ascii_case(target))
            .flat_map(|c| &c.entries)
            .filter(|entry| match &entry.kind {
                EntryKind::Message(_) | EntryKind::Action(_) | EntryKind::Notice(_) => true,
                EntryKind::Zirc(message) => {
                    matches!(message.event, Event::Text { .. } | Event::File(_))
                }
                _ => false,
            })
            .count()
    }

    /// Batches of history, and the conversations that have some.
    pub(super) fn batch(&mut self, line: &Line) -> Vec<Line> {
        let param = |i: usize| line.params.get(i).map_or("", String::as_str);
        match line.command.as_str() {
            "BATCH" => {
                let reference = param(0);
                if let Some(reference) = reference.strip_prefix('+') {
                    match param(1) {
                        "chathistory" => {
                            self.history.insert(reference.to_owned());
                            if let Some(gap) = self.gaps.get(&fold(param(2))) {
                                self.filling.insert(
                                    reference.to_owned(),
                                    Page {
                                        target: param(2).to_owned(),
                                        since: gap.since.clone(),
                                        count: 0,
                                        oldest: None,
                                        reached: false,
                                        newest: None,
                                        known: false,
                                    },
                                );
                            }
                        }
                        "draft/chathistory-targets" => {
                            self.targets.insert(reference.to_owned());
                        }
                        _ => {}
                    }
                } else if let Some(reference) = reference.strip_prefix('-') {
                    self.history.remove(reference);
                    self.targets.remove(reference);
                    if let Some(page) = self.filling.remove(reference) {
                        let target = page.target.clone();
                        // Once the history asked for is in, a message looked for may be too.
                        return self
                            .filled(page)
                            .or_else(|| self.seek(&target))
                            .into_iter()
                            .collect();
                    }
                }
                Vec::new()
            }
            "CHATHISTORY" if param(0) == "TARGETS" => {
                let target = param(1);
                // When something was last said there, for the list's order.
                if let Ok(at) = DateTime::parse_from_rfc3339(param(2))
                    && !target.is_empty()
                {
                    self.said_at(target, at.with_timezone(&Utc));
                }
                if is_channel(target) || target.is_empty() || is_service(target) {
                    return Vec::new();
                }
                let fresh = !self
                    .conversations
                    .iter()
                    .any(|c| super::target(&c.name).eq_ignore_ascii_case(target));
                // Our machines' history is mostly their agents', each with a conversation
                // of its own, made as it arrives.
                if !self.ours(target) {
                    self.ensure(target, Kind::Query);
                }
                if fresh {
                    // How far it's been read first, to know the invitations in it we've seen.
                    let mut lines: Vec<Line> = self.ask_read(target).into_iter().collect();
                    lines.push(self.latest(target));
                    lines
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }

    /// A message trimmed from the server's history, by us or anyone: it goes, with its
    /// reactions, wherever it is, and doesn't come back with history. One trimmed before it
    /// arrived stays away too.
    pub(super) fn trimmed(&mut self, line: &Line) -> Vec<Line> {
        let Some(crate::trim::Outcome::Trimmed { target, id, by }) = crate::trim::outcome(line)
        else {
            return Vec::new();
        };
        // As for reactions: a private one is ours with whoever isn't us.
        let name = if is_channel(&target) || by.eq_ignore_ascii_case(&self.me) {
            target
        } else {
            by
        };
        let named = self.position(&name);
        for (at, conversation) in self.conversations.iter_mut().enumerate() {
            if Some(at) == named || conversation.holds(&id) {
                conversation.trim(&id, &self.me);
            }
        }
        self.fresh
            .retain(|(_, entry)| entry.id.as_deref() != Some(id.as_str()));
        Vec::new()
    }
}
