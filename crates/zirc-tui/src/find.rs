//! Search (Alt-F), as every app has it: zirc-chat's [`Finding`], opened in the conversation
//! shown, laid out by [`Chat::search_screen`]. Typing searches, a token finished with a space
//! becomes a chip, Backspace in an empty box takes the last chip away, Tab searches
//! everywhere or back here, Up and Down pick a hit (or, with nothing typed, a recent search),
//! Enter opens it, Esc closes. The terminal searches what it has loaded: it doesn't ask the
//! organisation's history.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use zirc_core::chat::{Chat, Finding, Recent, SearchHit, SearchScreen};

use crate::rows::Zone;

/// Search, while it's open.
#[derive(Debug, Clone)]
pub struct Searching {
    /// The search, as zirc-chat keeps it.
    pub finding: Finding,
    /// The row picked: a hit, or a recent search while nothing's typed.
    at: usize,
    /// The search screen, laid out again only when the search or what's loaded changes
    /// ([`Searching::refresh`]): it reads everything loaded.
    screen: SearchScreen,
}

/// What a key did to search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Searched {
    /// Still open.
    Open,
    /// Closed.
    Closed,
    /// Open this hit; search stays open.
    Hit(Box<SearchHit>),
}

impl Searching {
    /// A search opened in the conversation `opened_in`, as zirc-chat starts one there, its
    /// times in `zone`.
    #[must_use]
    pub fn new(chat: &Chat, opened_in: Option<String>, zone: Zone) -> Self {
        let finding = Finding::new(chat.search_start(opened_in.as_deref()), opened_in);
        Self {
            screen: screen(chat, &finding, zone),
            finding,
            at: 0,
        }
    }

    /// The search screen, as zirc-chat laid it out when the search or what's loaded last
    /// changed.
    #[must_use]
    pub fn screen(&self) -> &SearchScreen {
        &self.screen
    }

    /// Lays the search screen out again, as what's loaded in `chat` changed, its times in
    /// `zone`.
    pub fn refresh(&mut self, chat: &Chat, zone: Zone) {
        self.screen = screen(chat, &self.finding, zone);
    }

    /// Whether nothing's typed nor chipped but the scope, when the recent searches list.
    #[must_use]
    pub fn blank(&self) -> bool {
        let query = self.finding.query();
        query.text.trim().is_empty() && query.from.is_none()
    }

    /// The row picked, among `count`.
    #[must_use]
    pub fn at(&self, count: usize) -> usize {
        self.at.min(count.saturating_sub(1))
    }

    /// Takes in `key`, with `chat`'s conversations in `zone` and the `recent` searches.
    pub fn key(
        &mut self,
        key: KeyEvent,
        (chat, zone): (&Chat, Zone),
        recent: &[Recent],
    ) -> Searched {
        let before = self.finding.query();
        let blank = self.blank();
        let count = if blank {
            recent.len()
        } else {
            self.screen.listed_hits().len()
        };
        let at = self.at(count);
        match key.code {
            KeyCode::Esc => return Searched::Closed,
            KeyCode::Enter if blank => {
                if let Some(recent) = recent.get(at) {
                    self.finding.recall(&recent.query);
                    self.at = 0;
                }
            }
            KeyCode::Enter => {
                if let Some(hit) = self.screen.listed_hits().get(at) {
                    return Searched::Hit(Box::new((*hit).clone()));
                }
            }
            KeyCode::Up => self.at = at.saturating_sub(1),
            KeyCode::Down => self.at = (at + 1).min(count.saturating_sub(1)),
            // Here or everywhere, as the control beside the box switches.
            KeyCode::Tab if self.finding.opened_in.is_some() => {
                let everywhere = chat.search_scope(&self.finding.query()).is_none();
                self.finding.everywhere(!everywhere);
                self.at = 0;
            }
            KeyCode::Backspace if self.finding.typed.is_empty() => {
                self.finding.backspace();
                self.at = 0;
            }
            KeyCode::Backspace => {
                let mut typed = self.finding.typed.clone();
                typed.pop();
                self.finding.take_typed(&typed);
                self.at = 0;
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let typed = format!("{}{c}", self.finding.typed);
                self.finding.take_typed(&typed);
                self.at = 0;
            }
            _ => {}
        }
        if self.finding.query() != before {
            self.refresh(chat, zone);
        }
        Searched::Open
    }
}

/// `finding`'s screen, as zirc-chat lays it out: what's loaded in `chat` only, its times in
/// `zone`. Online, as the terminal is while connected, so nothing says it's offline; it asks
/// no history, so nothing's counted elsewhere.
fn screen(chat: &Chat, finding: &Finding, zone: Zone) -> SearchScreen {
    chat.search_screen(
        &finding.query(),
        None,
        chat.is_connected(),
        finding.all_groups,
        |time| zone.wall(time),
    )
}
