//! The characters a coworker is drawn as: twelve people in zirc's ink style, big heads on
//! small bodies, picked from a gallery when a coworker is hired (the user's choice, 27 Nov
//! 2026). Each app bundles each one's still twice, a bust that fills its rounded square and a
//! full figure for the office's cards and the gallery, made once by zirc-theme-art
//! (`crates/zirc-theme-art/characters`). People keep their own photo; agent sessions that
//! aren't a coworker keep [`crate::AGENT_AVATARS`]' bots.

/// Each character by id, with its colour until someone picks another, a hue in degrees, one
/// of [`crate::roles::HUES`] and no two the same, and what it shows, for a screen reader and
/// the gallery, in the order the gallery offers them.
/// The Android app names the ids again, as a genrule must (`CHARACTERS` in
/// oss/apps/android/BUILD.bazel), and zirc-theme-art's `STILLS` lists their stills: change all three
/// together (their tests and genrule fail otherwise).
pub const CHARACTERS: [(&str, u16, &str); 12] = [
    (
        "char-01",
        210,
        "short blond hair, round glasses, a blue collared shirt",
    ),
    ("char-02", 30, "spiky black hair, an orange hoodie"),
    (
        "char-03",
        120,
        "curly hair, a beard, headphones, a green T-shirt",
    ),
    (
        "char-04",
        270,
        "a purple cap, a white shirt, purple overalls",
    ),
    (
        "char-05",
        330,
        "red hair in a bun, freckles, a wine top, a gold scarf",
    ),
    ("char-06", 180, "long black hair, a headset, a teal top"),
    (
        "char-07",
        60,
        "curly auburn hair, round glasses, a yellow hoodie",
    ),
    ("char-08", 240, "short black hair, a navy collared shirt"),
    (
        "char-09",
        150,
        "white hair in a bun, round glasses, a green top, a gold scarf",
    ),
    ("char-10", 0, "long dark hair, an orange T-shirt"),
    (
        "char-11",
        90,
        "spiky red hair, a white T-shirt, teal overalls",
    ),
    (
        "char-12",
        300,
        "an orange cap, a beard, a headset, a dark shirt",
    ),
];

/// `id` as one of [`CHARACTERS`], whatever its case; `None` for one that isn't.
#[must_use]
pub fn character(id: &str) -> Option<&'static str> {
    let id = id.trim();
    CHARACTERS
        .iter()
        .map(|(character, ..)| *character)
        .find(|character| character.eq_ignore_ascii_case(id))
}

/// Whether `id` is a picture a coworker or a role may be drawn with: one of [`CHARACTERS`],
/// or one of [`crate::AGENT_AVATARS`]' bots, as they were before the characters.
#[must_use]
pub fn is_picture(id: &str) -> bool {
    CHARACTERS.iter().any(|(character, ..)| *character == id)
        || crate::AGENT_AVATARS.iter().any(|(bot, _)| *bot == id)
}

/// `id` as a picture a line may say its agent session goes by ([`crate::Message::avatar`]),
/// whatever its case: one of [`crate::AGENT_AVATARS`]' bots or a picture from before them
/// ([`crate::agent_avatar`]), or one of [`CHARACTERS`], for a coworker's; `None` for neither.
#[must_use]
pub fn session_picture(id: &str) -> Option<&'static str> {
    crate::agent_avatar(id).or_else(|| character(id))
}

/// The colour `id` has until someone picks another ([`CHARACTERS`]); `None` for one that
/// isn't a character.
#[must_use]
pub fn hue(id: &str) -> Option<u16> {
    let id = character(id)?;
    CHARACTERS
        .iter()
        .find_map(|(character, hue, _)| (*character == id).then_some(*hue))
}

/// The characters as TypeScript, for the web apps' generated files: `COWORKER_CHARACTERS`.
#[must_use]
pub fn typescript() -> Vec<String> {
    vec![format!(
        "const COWORKER_CHARACTERS: readonly {{ id: string; hue: number; shows: string }}[] = [{}];",
        CHARACTERS
            .map(|(id, hue, shows)| format!("{{ id: {id:?}, hue: {hue}, shows: {shows:?} }}"))
            .join(", ")
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_character_has_its_own_colour_from_the_picker() {
        let mut hues: Vec<u16> = CHARACTERS.iter().map(|(_, hue, _)| *hue).collect();
        for hue in &hues {
            assert!(
                crate::roles::HUES.iter().any(|(offered, _)| offered == hue),
                "{hue}"
            );
        }
        hues.sort_unstable();
        hues.dedup();
        assert_eq!(hues.len(), CHARACTERS.len());
    }

    #[test]
    fn finds_a_character_by_id_and_a_name_its_own() {
        assert_eq!(character(" CHAR-07 "), Some("char-07"));
        assert_eq!(character("fixer"), None);
        assert_eq!(hue("char-10"), Some(0));
        assert_eq!(hue("char-13"), None);
    }

    #[test]
    fn a_picture_is_a_character_or_a_bot() {
        assert!(is_picture("char-12") && is_picture("fixer"));
        assert_eq!(session_picture("Char-03"), Some("char-03"));
        assert_eq!(session_picture("rocket"), Some("rocket"));
        assert_eq!(session_picture("unicorn"), None);
        assert!(!is_picture("CHAR-12") && !is_picture("unicorn"));
    }

    #[test]
    fn no_character_is_an_agents_bot() {
        for (id, ..) in CHARACTERS {
            assert!(crate::agent_avatar(id).is_none(), "{id}");
        }
    }
}
