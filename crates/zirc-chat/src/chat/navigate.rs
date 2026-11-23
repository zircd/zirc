//! Moving through the conversations by key, as every app does: Next and Previous
//! conversation step down and up the sidebar, and Next unread goes to what mentions us
//! first, both round the ends (the user's choice, 16 Nov 2026).

use std::collections::HashSet;

use super::{Chat, ListKind, ListOptions, fold};

impl Chat {
    /// The conversations the sidebar listed with `options` shows, top to bottom, each once:
    /// its rows that open a conversation we're in, the Server channel's among them.
    fn stepping_order(&self, options: ListOptions) -> Vec<String> {
        let mut seen = HashSet::new();
        self.sidebar_list(&options)
            .into_iter()
            .filter(|row| {
                matches!(row.kind, ListKind::Conversation | ListKind::Room)
                    && row.opens
                    && row.joined
                    && !row.name.is_empty()
            })
            .map(|row| row.name)
            .filter(|name| seen.insert(fold(name)))
            .collect()
    }

    /// Where the conversation shown is in `order`.
    fn shown_in(&self, order: &[String]) -> Option<usize> {
        let shown = &self.selected().name;
        order
            .iter()
            .position(|name| name.eq_ignore_ascii_case(shown))
    }

    /// Next conversation (`by` 1) or Previous (`by` -1): the conversation that many rows
    /// down or up the sidebar listed with `options` from the one shown, round the ends; the
    /// first or the last when the one shown isn't listed. None when nothing is.
    #[must_use]
    pub fn next_conversation(&self, options: &ListOptions, by: isize) -> Option<String> {
        let order = self.stepping_order(*options);
        let len = order.len().cast_signed();
        if len == 0 {
            return None;
        }
        let from = self
            .shown_in(&order)
            .map_or(if by > 0 { -1 } else { len }, usize::cast_signed);
        let at = (from + by).rem_euclid(len).cast_unsigned();
        Some(order[at].clone())
    }

    /// Next unread (`by` 1) or Previous unread (`by` -1): going down or up the whole
    /// sidebar from the conversation shown, round the ends, the first that mentions us, or
    /// else the first with anything unread. Folded parts are gone through too, as a fold
    /// keeps what's unread. None when nothing else is unread.
    #[must_use]
    pub fn next_unread(&self, by: isize) -> Option<String> {
        let order = self.stepping_order(ListOptions::default());
        let len = order.len().cast_signed();
        let from = self
            .shown_in(&order)
            .map_or(if by > 0 { -1 } else { len }, usize::cast_signed);
        let step = by.signum();
        let around: Vec<&String> = (1..=len)
            .map(|i| &order[(from + i * step).rem_euclid(len).cast_unsigned()])
            .filter(|name| !name.eq_ignore_ascii_case(&self.selected().name))
            .collect();
        let counted = |name: &&String, has: fn(&super::Conversation) -> bool| {
            self.conversation(name).is_some_and(has)
        };
        around
            .iter()
            .find(|name| counted(name, |c| c.mentions > 0))
            .or_else(|| around.iter().find(|name| counted(name, |c| c.unread > 0)))
            .map(|name| (*name).clone())
    }
}
