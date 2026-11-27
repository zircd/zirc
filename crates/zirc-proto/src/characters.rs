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
        "short blond hair, round glasses, a blue collared shirt, a lanyard, a mug",
    ),
    (
        "char-02",
        30,
        "spiky black hair, round glasses, an orange hoodie printed >_, headphones round his neck",
    ),
    (
        "char-03",
        120,
        "curly hair, a beard, headphones, a green T-shirt printed { }, a laptop with stickers",
    ),
    (
        "char-04",
        270,
        "a purple cap, round glasses, a white shirt, purple overalls printed #, a mug",
    ),
    (
        "char-05",
        330,
        "a green beanie, red hair, freckles, round glasses, a wine top, a gold scarf, a lanyard",
    ),
    (
        "char-06",
        180,
        "short black hair, stubble, a headset, a teal top printed </>, a lanyard, a laptop with stickers",
    ),
    (
        "char-07",
        60,
        "curly auburn hair, round glasses, a yellow hoodie printed λ, headphones round his neck",
    ),
    (
        "char-08",
        240,
        "an orange beanie, round glasses, a navy hoodie printed &&",
    ),
    (
        "char-09",
        150,
        "short white hair, a white beard, round glasses, a green top, a gold scarf, a mug",
    ),
    (
        "char-10",
        0,
        "curly dark hair, stubble, round glasses, an orange T-shirt printed $_, a lanyard, a laptop with stickers",
    ),
    (
        "char-11",
        90,
        "spiky red hair, round glasses, a white T-shirt, teal overalls printed 01, headphones round his neck",
    ),
    (
        "char-12",
        300,
        "an orange cap, a beard, a headset, a dark shirt printed 0x, a mug",
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

/// The character dealt to `who`, a session's id or a coworker's name, from those not in
/// `taken`, or from all of them when every one is: the same for the same `who` and `taken`,
/// so every app deals the same. A session at a hot desk is drawn as one (the user's choice,
/// 27 Nov 2026: a dealt character, never one a coworker of the office has), and so is a
/// coworker that hasn't picked one.
#[must_use]
pub fn deal<'a>(who: &str, taken: impl IntoIterator<Item = &'a str>) -> &'static str {
    let taken: Vec<&str> = taken.into_iter().collect();
    let free: Vec<&'static str> = CHARACTERS
        .iter()
        .map(|(character, ..)| *character)
        .filter(|character| !taken.contains(character))
        .collect();
    let from = if free.is_empty() {
        CHARACTERS.map(|(character, ..)| character).to_vec()
    } else {
        free
    };
    let hash = who.bytes().fold(2_166_136_261_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16_777_619)
    });
    from[usize::try_from(hash).unwrap_or_default() % from.len()]
}

/// The characters an office's coworkers are drawn as, by name, from each one's name and the
/// picture it picked, if any: the character it picked, else one dealt by its name from those
/// no other coworker picked or was dealt before it, in its name's order ([`deal`]). Every
/// app draws a coworker as it, and its server says it to zirc.dev.
#[must_use]
pub fn of_coworkers<'a>(
    coworkers: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
) -> Vec<(String, &'static str)> {
    let mut coworkers: Vec<(&str, Option<&str>)> = coworkers.into_iter().collect();
    coworkers.sort_by_key(|(name, _)| *name);
    let mut all: Vec<(String, &'static str)> = coworkers
        .iter()
        .filter_map(|(name, picked)| Some(((*name).to_owned(), character((*picked)?)?)))
        .collect();
    for (name, _) in &coworkers {
        if all.iter().any(|(known, _)| known == name) {
            continue;
        }
        let dealt = deal(name, all.iter().map(|(_, c)| *c));
        all.push(((*name).to_owned(), dealt));
    }
    all
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
    fn deals_the_same_character_every_time_and_none_thats_taken() {
        assert_eq!(deal("cc-598aa1", []), deal("cc-598aa1", []));
        let all: Vec<&str> = CHARACTERS.iter().map(|(id, ..)| *id).collect();
        let (taken, left) = all.split_at(11);
        assert_eq!(deal("cc-598aa1", taken.iter().copied()), left[0]);
        assert!(character(deal("cc-598aa1", all.iter().copied())).is_some());
        let dealt: Vec<&str> = (0..40).map(|n| deal(&format!("cc-{n}"), [])).collect();
        assert!(
            dealt.iter().any(|id| *id != dealt[0]),
            "spread over the twelve"
        );
    }

    #[test]
    fn no_character_is_an_agents_bot() {
        for (id, ..) in CHARACTERS {
            assert!(crate::agent_avatar(id).is_none(), "{id}");
        }
    }
}
