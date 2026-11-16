//! Offsets in text as the apps' own text counts them. zirc-chat counts in bytes; Swift's,
//! Kotlin's and JavaScript's strings count in UTF-16 code units, so a caret an app gives or
//! gets is converted here, once for every app.

/// The byte offset in `text` of `units` UTF-16 code units in: at a character's start, never
/// inside one, and at most its end.
#[must_use]
pub fn byte_offset(text: &str, units: u32) -> usize {
    let mut counted = 0u32;
    for (at, c) in text.char_indices() {
        if counted >= units {
            return at;
        }
        counted += u32::try_from(c.len_utf16()).unwrap_or(2);
    }
    text.len()
}

/// How many UTF-16 code units of `text` come before its byte offset `at`.
#[must_use]
pub fn utf16_offset(text: &str, at: usize) -> u32 {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    let units = text[..at].encode_utf16().count();
    u32::try_from(units).unwrap_or(u32::MAX)
}

/// An edit's result, `text` with the cursor at its byte offset `at`, as an app takes it: the
/// text and the cursor in UTF-16 code units.
#[must_use]
pub fn edited((text, at): (String, usize)) -> (String, u32) {
    let cursor = utf16_offset(&text, at);
    (text, cursor)
}

/// Text with a choice written in, such as a mention completed, as every app takes it back:
/// the text and where the caret goes, in UTF-16 code units. The phones read this record
/// through `UniFFI`, the web chat as JSON.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
pub struct Completed {
    /// The text now.
    pub text: String,
    /// Where the caret goes, after the choice and a space, in UTF-16 code units.
    pub cursor: u32,
}

impl Completed {
    /// An edit's text and its caret's byte offset ([`edited`]).
    #[must_use]
    pub fn of(edit: (String, usize)) -> Self {
        let (text, cursor) = edited(edit);
        Self { text, cursor }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_utf16_units_both_ways() {
        // 🎉 is four bytes and two UTF-16 units; é two bytes and one unit.
        let text = "🎉 é @ad";
        assert_eq!(byte_offset(text, 0), 0);
        assert_eq!(byte_offset(text, 2), 4);
        assert_eq!(byte_offset(text, 1), 4, "never inside a character");
        assert_eq!(byte_offset(text, 4), 7);
        assert_eq!(byte_offset(text, 99), text.len());
        assert_eq!(utf16_offset(text, 4), 2);
        assert_eq!(utf16_offset(text, 7), 4);
        assert_eq!(utf16_offset(text, text.len()), 8);
    }
}
