//! The input line: the text being typed, and where the cursor is.

/// The text being typed, with a cursor counted in characters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Input {
    text: String,
    cursor: usize,
}

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

    /// Takes the text to send, leaving the line empty.
    pub fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        self.cursor = 0;
        text
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
}
