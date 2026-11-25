//! The last thing said in a conversation, as every app's lists show it under the title,
//! as Discord does: `You: quick sync?`, `bob: yeah exactly`, `bob: 2 images`.

use chrono::{DateTime, Utc};
use zirc_proto::Event;

use super::conversation::{Entry, EntryKind, agent, is_channel};
use super::file::{Shape, shape};
use super::{Chat, fold};

/// What a list row previews: the last thing said, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    /// One line: who said it and what, `You: quick sync?`, `bob: 1 image`; an action as it
    /// reads, `bob waves`.
    pub text: String,
    /// When it was said.
    pub at: DateTime<Utc>,
}

impl Chat {
    /// What the list previews of the conversation called `name`: its last message, action,
    /// notice, file or agent event, by whoever said it, in a direct message too. Our own
    /// read `You: …`; anyone else's by the name their rows head them with
    /// ([`Chat::speaker`]), but for the person or agent a one-to-one conversation is with,
    /// whom its title names already: `lunch at noon?`, as a narrow row has no room for both. An
    /// action keeps its name, as it reads as a sentence. Files by how many, `1 image`, `2 files`; an agent's event by
    /// its headline. `None` before anything's said.
    #[must_use]
    pub fn preview(&self, name: &str) -> Option<Preview> {
        let conversation = self.conversation(name)?;
        let entries = &conversation.entries;
        let at = entries
            .iter()
            .rposition(|entry| conversation.lists(entry))?;
        let entry = &entries[at];
        let who = if fold(&entry.from) == fold(&self.me) {
            "You".to_owned()
        } else {
            self.speaker(entry).name
        };
        let line = |text: &str| {
            super::strip_formatting(text.lines().next().unwrap_or_default())
                .trim()
                .to_owned()
        };
        // The person a one-to-one conversation is with goes unnamed: its title names them, as
        // an agent's own conversation names it. A machine's own conversation holds its
        // agents', so each is named.
        let theirs = !is_channel(name)
            && match agent(name) {
                Some((nick, _)) => fold(&entry.from) == fold(nick),
                None => self.machine(name).is_none() && fold(&entry.from) == fold(name),
            };
        let named = |what: String| {
            if theirs {
                what
            } else {
                format!("{who}: {what}")
            }
        };
        let text = match &entry.kind {
            EntryKind::Message(text) | EntryKind::Notice(text) => named(line(text)),
            EntryKind::Action(text) => format!("{who} {}", line(text)),
            EntryKind::Zirc(message) => match &message.event {
                Event::File(_) => named(files(&entries[..=at])),
                Event::Text { body } => named(line(body)),
                // A vacant role's answer by its card's heading, whatever its plain words.
                event if let Some(vacant) = zirc_proto::roles::Vacant::of(event) => {
                    named(vacant.title())
                }
                event => named(super::describe::describe(event).headline),
            },
            _ => return None,
        };
        Some(Preview {
            text,
            at: entry.time,
        })
    }

    /// The previews of the conversations called `names`: the newest of them, as a row
    /// holding several, a person and their agents', previews.
    #[must_use]
    pub fn newest_preview<'a>(&self, names: impl IntoIterator<Item = &'a str>) -> Option<Preview> {
        names
            .into_iter()
            .filter_map(|name| self.preview(name))
            .max_by_key(|preview| preview.at)
    }
}

/// The files `entries` end with, from the last one's sender, as a preview counts them:
/// `1 image`, `3 images`, `2 files` (any that isn't an image makes them files).
fn files(entries: &[Entry]) -> String {
    let Some(last) = entries.last() else {
        return String::new();
    };
    let shapes: Vec<Shape> = entries
        .iter()
        .rev()
        .take_while(|entry| fold(&entry.from) == fold(&last.from))
        .map_while(|entry| match &entry.kind {
            EntryKind::Zirc(message) => match &message.event {
                Event::File(file) => Some(shape(file)),
                _ => None,
            },
            _ => None,
        })
        .collect();
    let images = shapes.iter().all(|shape| *shape == Shape::Image);
    match (shapes.len(), images) {
        (1, true) => "1 image".to_owned(),
        (1, false) => "1 file".to_owned(),
        (n, true) => format!("{n} images"),
        (n, false) => format!("{n} files"),
    }
}
