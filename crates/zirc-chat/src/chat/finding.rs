//! A search open, as every app keeps it without drawing it: where it was opened, its chips,
//! what's typed after them, and what the organisation's history found. The desktop app, the
//! phones (through zirc-app) and the web chat all change it through these, and lay it out with
//! [`Chat::search_screen`]; what an app keeps across searches (its recent searches, the orders
//! chosen) is the app's.

use super::{
    Chat, ScreenChip, SearchLimits, SearchQuery, SearchScope, SearchScreen, SearchSegments,
    SearchSort, ServerSearch, absorb_search, compose_search, search_segments, segment_scope,
};

/// How long typing settles before the history is asked, in milliseconds, in every app.
pub const SEARCH_SETTLE_MS: u32 = 300;

/// A search open: where it was opened, its chips, what's typed after them, and what the
/// history found.
#[derive(Debug, Clone)]
pub struct Finding {
    /// The conversation search was opened in, whose control it shows beside Everywhere:
    /// none for one opened everywhere.
    pub opened_in: Option<String>,
    /// The chips: the scope and the filters, with nothing typed.
    pub base: SearchQuery,
    /// What's typed after them.
    pub typed: String,
    /// What the history found for it.
    pub server: Option<ServerSearch>,
    /// Whether every conversation's group shows, not only the first few.
    pub all_groups: bool,
}

impl Finding {
    /// A search starting as `base`, opened in the conversation `opened_in` (none everywhere),
    /// with nothing typed.
    #[must_use]
    pub fn new(base: SearchQuery, opened_in: Option<String>) -> Self {
        Self {
            opened_in,
            base,
            typed: String::new(),
            server: None,
            all_groups: false,
        }
    }

    /// The search the box holds: its chips and what's typed. Text that won't read as a
    /// search, such as `after:yesterday`, is looked for as it is, and
    /// [`Finding::problem`] says why.
    #[must_use]
    pub fn query(&self) -> SearchQuery {
        compose_search(&self.base, &self.typed).unwrap_or_else(|_| SearchQuery {
            text: self.typed.trim().to_owned(),
            ..self.base.clone()
        })
    }

    /// Why what's typed doesn't read as a search, such as a day that isn't one; none when it
    /// does.
    #[must_use]
    pub fn problem(&self) -> Option<String> {
        compose_search(&self.base, &self.typed)
            .err()
            .map(|why| why.to_string())
    }

    /// Changes the chips to `base`, keeping what's typed.
    pub fn rebase(&mut self, base: SearchQuery) {
        self.base = base;
        self.all_groups = false;
    }

    /// Takes in what the box holds now, `typed`: tokens finished with a space become chips.
    pub fn take_typed(&mut self, typed: &str) {
        if let Some((base, rest)) = absorb_search(&self.base, typed) {
            self.rebase(base);
            self.typed = rest;
        } else {
            typed.clone_into(&mut self.typed);
            self.all_groups = false;
        }
    }

    /// Searches where `scope` says, keeping the chips and what's typed.
    pub fn scope(&mut self, scope: SearchScope) {
        let mut base = self.base.clone();
        base.scope = scope;
        self.rebase(base);
    }

    /// Searches everywhere, or back in the conversation search was opened in: the
    /// segmented control ([`SearchSegments`], [`segment_scope`]).
    pub fn everywhere(&mut self, everywhere: bool) {
        self.scope(segment_scope(self.opened_in.as_deref(), everywhere));
    }

    /// Searches everywhere: the widening row, or the scope chip's ×.
    pub fn widen(&mut self) {
        let base = self.base.widened();
        self.rebase(base);
    }

    /// Takes the chip for `token` away; without its scope chip it searches everywhere.
    pub fn remove(&mut self, token: &str) {
        if let Some(chip) = self.base.chips().into_iter().find(|c| c.token == token) {
            let base = self.base.without(&chip);
            self.rebase(base);
        }
    }

    /// Adds `token` (`in:#api` from a group's count, `from:leo`) as a chip, keeping what's
    /// typed.
    pub fn add(&mut self, token: &str) {
        if let Ok(base) = SearchQuery::parse(&format!("{} {token}", self.base), None) {
            self.rebase(base);
        }
    }

    /// Backspace in an empty box: takes the last chip away, the scope last of all, but never
    /// one the control beside the box holds. Whether one was.
    pub fn backspace(&mut self) -> bool {
        if !self.typed.is_empty() {
            return false;
        }
        // The scope stays where the control beside the box holds it: in the conversation
        // search was opened in.
        let held = self
            .opened_in
            .as_deref()
            .is_some_and(|here| self.base.scope == segment_scope(Some(here), false));
        let chips = self.base.chips();
        let Some(chip) = chips
            .iter()
            .rev()
            .find(|chip| !(held && chip.token.starts_with("in:")))
        else {
            return false;
        };
        let base = self.base.without(chip);
        self.rebase(base);
        true
    }

    /// Orders the results `sort`.
    pub fn sort(&mut self, sort: SearchSort) {
        let mut base = self.base.clone();
        base.sort = Some(sort);
        self.rebase(base);
    }

    /// Shows every conversation's group, not only the first few: "and 2 more conversations".
    pub fn show_all_groups(&mut self) {
        self.all_groups = true;
    }

    /// Recalls `query`, a recent search: its chips, and its words typed.
    pub fn recall(&mut self, query: &str) {
        if let Ok(recalled) = SearchQuery::parse(query, None) {
            self.typed.clone_from(&recalled.text);
            self.rebase(SearchQuery {
                text: String::new(),
                ..recalled
            });
        }
    }

    /// What the history is to be asked for, for `query` (the search as it is now, in the
    /// order an app chose): made anew, unasked, when the search has changed.
    pub fn server_for(&mut self, chat: &Chat, query: &SearchQuery) -> &mut ServerSearch {
        if self
            .server
            .as_ref()
            .is_none_or(|server| server.query != *query)
        {
            self.server = Some(chat.server_search(query));
        }
        self.server.get_or_insert_with(|| chat.server_search(query))
    }

    /// The segmented control over `screen`, for a search opened in the conversation the
    /// sidebar names `here`; none for one opened everywhere, whose scope is a chip
    /// ([`search_segments`]).
    #[must_use]
    pub fn segments(&self, screen: &SearchScreen, here: Option<String>) -> Option<SearchSegments> {
        search_segments(screen, self.opened_in.as_deref(), here)
    }

    /// What the search couldn't cover, of `limits` ([`SearchScreen::limits`]), said once:
    /// when the segmented control greys Everywhere out (`segments`), it says why beneath it,
    /// offline, so the limits line doesn't say so again.
    #[must_use]
    pub fn limits(
        limits: Option<SearchLimits>,
        segments: Option<&SearchSegments>,
    ) -> Option<SearchLimits> {
        limits.filter(|_| segments.is_none_or(|segments| segments.everywhere_unavailable.is_none()))
    }

    /// The chips in the box, of `chips` ([`SearchScreen::chips`]): not the scope's when the
    /// segmented control shows in its place (`segmented`).
    #[must_use]
    pub fn chips(chips: Vec<ScreenChip>, segmented: bool) -> Vec<ScreenChip> {
        chips
            .into_iter()
            .filter(|chip| !(chip.scope && segmented))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(opened_in: Option<&str>) -> Finding {
        Finding::new(
            SearchQuery::parse("", opened_in).unwrap(),
            opened_in.map(str::to_owned),
        )
    }

    #[test]
    fn a_finished_token_becomes_a_chip_and_the_rest_stays_typed() {
        let mut search = finding(Some("#api"));
        search.take_typed("from:leo time");
        assert_eq!(search.typed, "time");
        assert_eq!(search.base.from.as_deref(), Some("leo"));
        assert_eq!(search.query().to_string(), "in:#api from:leo time");
    }

    #[test]
    fn widens_narrows_and_takes_chips_away() {
        let mut search = finding(Some("#api"));
        search.take_typed("timeout");
        search.scope(SearchScope::Server);
        assert_eq!(search.query().to_string(), "in:* timeout");
        search.add("in:#web");
        assert_eq!(search.query().to_string(), "in:#web timeout");
        search.add("from:leo");
        search.remove("in:#web");
        assert_eq!(search.query().to_string(), "in:* from:leo timeout");
        search.remove("from:leo");
        assert_eq!(search.query().to_string(), "in:* timeout");
    }

    #[test]
    fn a_recent_search_comes_back_with_its_chips() {
        let mut search = finding(None);
        search.recall("in:#api sort:best timeout");
        assert_eq!(search.typed, "timeout");
        assert_eq!(search.base.scope, SearchScope::Conversation("#api".into()));
        assert_eq!(search.base.sort, Some(SearchSort::Best));
    }

    #[test]
    fn what_wont_read_is_looked_for_as_typed_and_said_why() {
        let mut search = finding(Some("#api"));
        assert_eq!(search.problem(), None);
        search.take_typed("after:someday");
        assert!(search.problem().is_some());
        assert_eq!(search.query().text, "after:someday");
    }

    #[test]
    fn backspace_takes_the_last_chip_but_not_the_scope_the_control_holds() {
        let mut search = finding(Some("#api"));
        search.take_typed("from:leo ");
        assert!(search.backspace());
        assert_eq!(search.base.from, None);
        assert!(!search.backspace());
        search.everywhere(true);
        search.add("in:#web");
        assert!(search.backspace(), "a scope chip of its own goes");
        search.take_typed("x");
        assert!(!search.backspace(), "not while something's typed");
    }
}
