//! What the message field completes as a person types, decided once for every app: an `@`
//! mention ([`chat::mention_query`]) before a `:shortcode` ([`emoji::shortcode_query`]), so
//! each app only draws the list it's told to.

use crate::{chat, emoji};

/// What's being typed at the cursor that the composer offers to complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completing<'a> {
    /// An `@` mention: what's typed after the `@`, for [`chat::Chat::mentions`].
    Mention(&'a str),
    /// A `:shortcode`: what's typed after the `:`, for [`emoji::suggest`].
    Shortcode(&'a str),
}

/// What's being typed in `text` at `cursor`, a byte offset, that the composer completes: an
/// `@` mention first, else a `:shortcode`; `None` for anything else.
#[must_use]
pub fn completing(text: &str, cursor: usize) -> Option<Completing<'_>> {
    if let Some((_, prefix)) = chat::mention_query(text, cursor) {
        return Some(Completing::Mention(prefix));
    }
    emoji::shortcode_query(text, cursor).map(|(_, query)| Completing::Shortcode(query))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completes_a_mention_before_a_shortcode() {
        assert_eq!(completing("hi @ad", 6), Some(Completing::Mention("ad")));
        assert_eq!(completing("hi :tad", 7), Some(Completing::Shortcode("tad")));
        // Past a mention, a shortcode after it is what's being typed.
        assert_eq!(
            completing("hi @ad :jo", 10),
            Some(Completing::Shortcode("jo"))
        );
        assert_eq!(completing("hi there", 8), None);
    }
}
