//! A nick's avatar as every app draws it: up to two letters, on a square of the nick's own
//! colour, the same for the same nick everywhere. (The desktop app's, moved here so the
//! apps share it.)

/// A nick's hue in whole degrees, from 0 up to 359, the same every time for the same nick,
/// so each person's colour is the same in every app. Every app's colour for a nick or an
/// agent session is this one.
#[must_use]
pub fn nick_degrees(nick: &str) -> u16 {
    let hash = nick.bytes().fold(2_166_136_261_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16_777_619)
    });
    u16::try_from(hash % 360).unwrap_or_default()
}

/// Up to two letters for an avatar, from a nick or the name someone shows as: the first of
/// each of its first two parts, as `BL` for `bob-laptop` and `AL` for `Ada Lovelace`, or its
/// first letter alone.
#[must_use]
pub fn initials(nick: &str) -> String {
    let parts: Vec<&str> = nick
        .split(|c: char| c.is_whitespace() || matches!(c, '-' | '_' | '.' | '/' | '|'))
        .filter(|part| !part.is_empty())
        .collect();
    let first = |part: &str| part.chars().find(|c| c.is_alphanumeric());
    let letters: String = match parts.as_slice() {
        [one, two, ..] => first(one).into_iter().chain(first(two)).collect(),
        [one] => first(one).into_iter().collect(),
        [] => String::new(),
    };
    letters.to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_initials() {
        assert_eq!(initials("bob-laptop"), "BL");
        assert_eq!(initials("mattias"), "M");
        assert_eq!(initials("cc_3f2a"), "C3");
        assert_eq!(initials("-"), "");
        assert_eq!(initials("Ada Lovelace"), "AL");
    }

    #[test]
    fn gives_a_nick_the_same_hue_every_time() {
        assert_eq!(nick_degrees("alice"), nick_degrees("alice"));
        assert!(nick_degrees("alice") < 360);
        assert_ne!(nick_degrees("alice"), nick_degrees("bob"));
    }
}
