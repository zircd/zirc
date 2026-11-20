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
        self.keep_draft(self.selected, text);
    }

    /// [`Chat::draft`] of the conversation `name`, for an app whose composer belongs to one
    /// by name, as the phones' do: empty for one there isn't.
    #[must_use]
    pub fn draft_in(&self, name: &str) -> &str {
        self.position(name)
            .map_or("", |at| &self.conversations[at].draft)
    }

    /// [`Chat::set_draft`] in the conversation `name`; nothing for one there isn't.
    pub fn set_draft_in(&mut self, name: &str, text: &str) {
        if let Some(at) = self.position(name) {
            self.keep_draft(at, text);
        }
    }

    /// Keeps `text` as the draft of the conversation at `at`.
    fn keep_draft(&mut self, at: usize, text: &str) {
        let conversation = &mut self.conversations[at];
        if conversation.draft != text {
            text.clone_into(&mut conversation.draft);
        }
    }
}
