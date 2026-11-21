//! Jump to (Alt+K), as every app has it: what's typed, and the row picked among what
//! zirc-chat lays out for it ([`Chat::jump_to`]): the conversations it finds, the channels to
//! join, then Join or Create #name. Up and Down move, Enter opens the row picked, Esc closes.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use zirc_core::chat::{Chat, JumpTo};

/// Jump to, while it's open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Jumping {
    /// What's typed.
    pub query: String,
    /// The row picked, among [`names`]'s.
    at: usize,
}

/// What a key did to Jump to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Jumped {
    /// Still open.
    Open,
    /// Closed without going anywhere.
    Closed,
    /// Go to the conversation or channel by this name, joining or making a channel we're
    /// not in ([`Chat::open`]).
    To(String),
}

/// What [`Chat::jump_to`] offers to go to, by name, top to bottom: its conversations, the
/// channels to join, and Join or Create #name.
#[must_use]
pub fn names(jump: &JumpTo) -> Vec<&str> {
    jump.conversations
        .iter()
        .map(|row| row.name.as_str())
        .chain(jump.browse.iter().map(|row| row.name.as_str()))
        .chain(jump.offer.iter().map(|offer| offer.name.as_str()))
        .collect()
}

impl Jumping {
    /// The row picked, among `count`.
    #[must_use]
    pub fn at(&self, count: usize) -> usize {
        self.at.min(count.saturating_sub(1))
    }

    /// Takes in `key`, with what `chat` offers for what's typed.
    pub fn key(&mut self, key: KeyEvent, chat: &Chat) -> Jumped {
        let jump = chat.jump_to(&self.query);
        let names = names(&jump);
        let at = self.at(names.len());
        match key.code {
            KeyCode::Esc => return Jumped::Closed,
            KeyCode::Enter => {
                return names
                    .get(at)
                    .map_or(Jumped::Open, |name| Jumped::To((*name).to_owned()));
            }
            KeyCode::Up => self.at = at.saturating_sub(1),
            KeyCode::Down => self.at = (at + 1).min(names.len().saturating_sub(1)),
            KeyCode::Backspace => {
                self.query.pop();
                self.at = 0;
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.query.push(c);
                self.at = 0;
            }
            _ => {}
        }
        Jumped::Open
    }
}
