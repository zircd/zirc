//! What's written in each conversation's composer and not yet sent, kept here for every
//! app (the user's choice, 19 Nov 2026): switching away and back finds it again, and it
//! never follows into the conversation opened next. In memory only, so a restart drops it.

use super::Chat;

impl Chat {
    /// What's written in the shown conversation's composer and not yet sent: what an app
    /// puts in its composer as a conversation is shown.
    #[must_use]
    pub fn draft(&self) -> &str {
        &self.selected().draft
    }

    /// Keeps `text` as what's written in the shown conversation's composer: each app says
    /// so as it changes, and once it's sent, empty.
    pub fn set_draft(&mut self, text: &str) {
        let shown = &mut self.conversations[self.selected];
        if shown.draft != text {
            text.clone_into(&mut shown.draft);
        }
    }
}
