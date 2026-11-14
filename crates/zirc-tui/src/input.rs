//! The input line: the text being typed, where the cursor is, and what was sent before.

/// The text being typed, with a cursor counted in characters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Input {
    text: String,
    cursor: usize,
    history: Vec<String>,
    /// While browsing history: which entry, and what was being typed before.
    browsing: Option<(usize, String)>,
}

/// How many sent lines Up and Down go back through.
const HISTORY: usize = 200;

impl Input {
    /// The text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The cursor, in characters from the start.
    #[must_use]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The cursor, in bytes from the start, as zirc-chat's mention helpers count.
    #[must_use]
    pub fn cursor_byte(&self) -> usize {
        self.byte(self.cursor)
    }

    /// Replaces the text with `text`, the cursor at the byte `at`.
    pub fn replace(&mut self, text: String, at: usize) {
        self.cursor = text[..at.min(text.len())].chars().count();
        self.text = text;
    }

    /// Types `c` at the cursor.
    pub fn insert(&mut self, c: char) {
        let at = self.byte(self.cursor);
        self.text.insert(at, c);
        self.cursor += 1;
    }

    /// Types `text` at the cursor.
    pub fn insert_str(&mut self, text: &str) {
        for c in text.chars() {
            self.insert(c);
        }
    }

    /// Deletes the character before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            let at = self.byte(self.cursor);
            self.text.remove(at);
        }
    }

    /// Deletes the character under the cursor.
    pub fn delete(&mut self) {
        let at = self.byte(self.cursor);
        if at < self.text.len() {
            self.text.remove(at);
        }
    }

    /// Moves the cursor one character left.
    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Moves the cursor one character right.
    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.len());
    }

    /// Moves the cursor to the start.
    pub fn home(&mut self) {
        self.cursor = 0;
    }

    /// Moves the cursor to the end.
    pub fn end(&mut self) {
        self.cursor = self.len();
    }

    /// Deletes from the start to the cursor.
    pub fn kill_to_start(&mut self) {
        let at = self.byte(self.cursor);
        self.text.drain(..at);
        self.cursor = 0;
    }

    /// Deletes from the cursor to the end.
    pub fn kill_to_end(&mut self) {
        let at = self.byte(self.cursor);
        self.text.truncate(at);
    }

    /// Deletes the word before the cursor, and the spaces after it.
    pub fn kill_word(&mut self) {
        let end = self.byte(self.cursor);
        let before = self.text[..end].trim_end_matches(' ');
        let start = before.rfind(' ').map_or(0, |at| at + 1);
        self.cursor -= self.text[start..end].chars().count();
        self.text.drain(start..end);
    }

    /// The word the cursor is at the end of, and where it starts in characters.
    #[must_use]
    pub fn word_before_cursor(&self) -> (usize, &str) {
        let end = self.byte(self.cursor);
        let start = self.text[..end].rfind(' ').map_or(0, |at| at + 1);
        (self.text[..start].chars().count(), &self.text[start..end])
    }

    /// Replaces the word the cursor is at the end of with `word`.
    pub fn replace_word_before_cursor(&mut self, word: &str) {
        let (start, _) = self.word_before_cursor();
        let (from, to) = (self.byte(start), self.byte(self.cursor));
        self.text.replace_range(from..to, word);
        self.cursor = start + word.chars().count();
    }

    /// Takes the text to send, remembering it for Up.
    pub fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        self.cursor = 0;
        self.browsing = None;
        if !text.trim().is_empty() && self.history.last() != Some(&text) {
            self.history.push(text.clone());
            if self.history.len() > HISTORY {
                self.history.remove(0);
            }
        }
        text
    }

    /// Shows the line sent before the one shown.
    pub fn older(&mut self) {
        let at = match &self.browsing {
            Some((at, _)) => at.checked_sub(1),
            None => self.history.len().checked_sub(1),
        };
        let Some(at) = at else { return };
        let typed = match self.browsing.take() {
            Some((_, typed)) => typed,
            None => self.text.clone(),
        };
        self.browsing = Some((at, typed));
        self.set(self.history[at].clone());
    }

    /// Shows the line sent after the one shown, and then what was being typed.
    pub fn newer(&mut self) {
        let Some((at, typed)) = self.browsing.take() else {
            return;
        };
        if at + 1 < self.history.len() {
            self.browsing = Some((at + 1, typed));
            self.set(self.history[at + 1].clone());
        } else {
            self.set(typed);
        }
    }

    fn set(&mut self, text: String) {
        self.text = text;
        self.cursor = self.len();
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    fn byte(&self, chars: usize) -> usize {
        self.text
            .char_indices()
            .nth(chars)
            .map_or(self.text.len(), |(at, _)| at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> Input {
        let mut input = Input::default();
        input.insert_str(text);
        input
    }

    #[test]
    fn edits_at_the_cursor() {
        let mut input = typed("héllo");
        input.left();
        input.left();
        input.insert('X');
        assert_eq!(input.text(), "hélXlo");
        input.backspace();
        input.home();
        input.delete();
        assert_eq!(input.text(), "éllo");
        input.right();
        input.kill_to_end();
        assert_eq!(input.text(), "é");
    }

    #[test]
    fn kills_words() {
        let mut input = typed("one two  ");
        input.kill_word();
        assert_eq!((input.text(), input.cursor()), ("one ", 4));
        input.kill_word();
        assert_eq!(input.text(), "");
    }

    #[test]
    fn completes_the_word_before_the_cursor() {
        let mut input = typed("hi al");
        assert_eq!(input.word_before_cursor(), (3, "al"));
        input.replace_word_before_cursor("alice");
        assert_eq!((input.text(), input.cursor()), ("hi alice", 8));
    }

    #[test]
    fn goes_through_history_and_back_to_what_was_typed() {
        let mut input = Input::default();
        for line in ["first", "second"] {
            input.insert_str(line);
            input.take();
        }
        input.insert_str("draft");
        input.older();
        assert_eq!(input.text(), "second");
        input.older();
        input.older();
        assert_eq!(input.text(), "first");
        input.newer();
        assert_eq!(input.text(), "second");
        input.newer();
        assert_eq!(input.text(), "draft");
    }
}
