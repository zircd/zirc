//! A reaction picker's contents, for every app alike: Unicode's emoji from the `emojis` crate,
//! in its groups, found by name or shortcode, with the ones the person used lately first. The
//! apps draw [`sections`] or [`rows`]; nothing here knows about a UI, or where an app keeps
//! [`Recent`].

use emojis::{Emoji, Group};
use serde::{Deserialize, Serialize};

/// How many recently used emoji to remember.
pub const RECENT: usize = 18;

/// How many reactions a message's hover bar offers at a click, before the picker.
pub const QUICK: usize = 5;

/// What the hover bar offers until the person has given that many reactions of their own.
pub const QUICK_DEFAULTS: [&str; 6] = ["👍", "❤️", "😂", "🎉", "👀", "✅"];

/// What a picker says when no emoji is called what's typed.
pub const NO_MATCH: &str = "No emoji match.";

/// What a picker's search field says while it's empty.
pub const SEARCH: &str = "Search emoji";

/// The newest emoji an app can draw, as Unicode's emoji version: (15, 0) for Emoji 15.0.
/// Newer ones are left out, as the system's font would show them as boxes.
pub type Newest = (u32, u32);

/// A picker section's heading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// What the person used lately.
    Recent,
    /// What matches the search.
    Results,
    /// One of Unicode's groups.
    Group(Group),
}

impl Section {
    /// The heading, as a picker shows it.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Section::Recent => "Recently used",
            Section::Results => "Results",
            Section::Group(group) => group_title(group),
        }
    }
}

/// Unicode's group names, shortened as pickers do.
#[must_use]
pub fn group_title(group: Group) -> &'static str {
    match group {
        Group::SmileysAndEmotion => "Smileys",
        Group::PeopleAndBody => "People",
        Group::AnimalsAndNature => "Nature",
        Group::FoodAndDrink => "Food",
        Group::TravelAndPlaces => "Travel",
        Group::Activities => "Activities",
        Group::Objects => "Objects",
        Group::Symbols => "Symbols",
        Group::Flags => "Flags",
    }
}

/// Unicode's groups, in its order, for a picker's tabs.
pub fn groups() -> impl Iterator<Item = Group> {
    Group::iter()
}

/// One row of a picker's grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A section's heading.
    Heading(Section),
    /// Up to a row's worth of emoji.
    Emoji(Vec<&'static str>),
}

fn drawable(emoji: &Emoji, newest: Newest) -> bool {
    let version = emoji.emoji_version();
    (version.major(), version.minor()) <= newest
}

/// Whether `emoji` matches `query`: every word of it starts a word of the emoji's name or of
/// one of its shortcodes, so "thu up" finds 👍.
fn matches(emoji: &Emoji, query: &[String]) -> bool {
    let name = emoji.name().to_lowercase();
    let words: Vec<&str> = name
        .split([' ', '-', ':'])
        .chain(emoji.shortcodes().flat_map(|code| code.split('_')))
        .collect();
    query
        .iter()
        .all(|q| words.iter().any(|word| word.starts_with(q.as_str())))
}

/// A picker's sections, none of them empty: with a `query`, what matches it; without one, the
/// `recent` emoji and then every group. Only emoji up to `newest` are in them.
pub fn sections(
    query: &str,
    recent: &[String],
    newest: Newest,
) -> Vec<(Section, Vec<&'static str>)> {
    let query: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut sections = Vec::new();
    let mut section = |section: Section, emoji: Vec<&'static str>| {
        if !emoji.is_empty() {
            sections.push((section, emoji));
        }
    };
    if !query.is_empty() {
        section(
            Section::Results,
            emojis::iter()
                .filter(|e| drawable(e, newest) && matches(e, &query))
                .map(Emoji::as_str)
                .collect(),
        );
        return sections;
    }
    section(
        Section::Recent,
        recent
            .iter()
            .filter_map(|e| emojis::get(e))
            .filter(|e| drawable(e, newest))
            .map(Emoji::as_str)
            .collect(),
    );
    for group in groups() {
        section(
            Section::Group(group),
            group
                .emojis()
                .filter(|e| drawable(e, newest))
                .map(Emoji::as_str)
                .collect(),
        );
    }
    sections
}

/// [`sections`] as a grid's rows, `per_row` emoji to a row.
#[must_use]
pub fn rows(query: &str, recent: &[String], newest: Newest, per_row: usize) -> Vec<Row> {
    sections(query, recent, newest)
        .into_iter()
        .flat_map(|(section, emoji)| {
            std::iter::once(Row::Heading(section)).chain(
                emoji
                    .chunks(per_row.max(1))
                    .map(|chunk| Row::Emoji(chunk.to_vec()))
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

/// The row a section's heading is on, for jumping to it.
#[must_use]
pub fn heading_row(rows: &[Row], section: Section) -> Option<usize> {
    rows.iter().position(|row| *row == Row::Heading(section))
}

/// The emoji's name, for a tooltip, a screen reader or a script.
pub fn name(emoji: &str) -> Option<&'static str> {
    emojis::get(emoji).map(Emoji::name)
}

/// The emoji `typed` means, as a command such as `/react` takes it: the emoji itself, a
/// shortcode with or without its colons (`tada`, `:+1:`), or else the first whose name
/// matches the words, as a search would find it. Only emoji up to `newest`.
#[must_use]
pub fn find(typed: &str, newest: Newest) -> Option<&'static str> {
    let typed = typed.trim();
    let code = typed.trim_matches(':');
    emojis::get(typed)
        .or_else(|| emojis::get_by_shortcode(code))
        .filter(|e| drawable(e, newest))
        .map(Emoji::as_str)
        .or_else(|| match &sections(code, &[], newest)[..] {
            [(Section::Results, found), ..] => found.first().copied(),
            _ => None,
        })
}

/// The emoji the person used lately, newest first. Each app keeps them where it keeps its own
/// state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recent {
    /// The emoji, reactions and those written in messages alike.
    pub emoji: Vec<String>,
    /// The reactions the person gave, newest first.
    pub reactions: Vec<String>,
}

impl Recent {
    /// Puts `emoji` first.
    pub fn used(&mut self, emoji: &str) {
        first(&mut self.emoji, emoji);
    }

    /// Puts `emoji` first among the reactions given, and the emoji used.
    pub fn reacted(&mut self, emoji: &str) {
        self.used(emoji);
        first(&mut self.reactions, emoji);
    }

    /// The `count` reactions to offer at a click: those the person gave last, newest first,
    /// then [`QUICK_DEFAULTS`], each once.
    #[must_use]
    pub fn quick(&self, count: usize) -> Vec<String> {
        then_defaults(&self.reactions, count)
    }

    /// The `count` emoji a composer's short menu offers to write: those the person used
    /// lately, in messages or as reactions, newest first, then [`QUICK_DEFAULTS`], each once.
    #[must_use]
    pub fn to_write(&self, count: usize) -> Vec<String> {
        then_defaults(&self.emoji, count)
    }
}

/// `first`, then [`QUICK_DEFAULTS`], each once, at most `count`.
fn then_defaults(first: &[String], count: usize) -> Vec<String> {
    let mut offered: Vec<String> = Vec::with_capacity(count);
    for emoji in first.iter().map(String::as_str).chain(QUICK_DEFAULTS) {
        if offered.len() == count {
            break;
        }
        if !offered.iter().any(|seen| seen == emoji) {
            offered.push(emoji.to_owned());
        }
    }
    offered
}

/// How many letters after a `:` before emoji are offered, so `re:` and times aren't.
pub const SHORTCODE_AFTER: usize = 2;

/// The `:shortcode` being typed in `text` at `cursor`, a byte offset: where its `:` is and
/// what's typed after it up to the cursor, `(3, "tad")` for `"hi :tad"` at its end. `None`
/// when the cursor isn't in one: the `:` counts only at the start or after a space, and
/// [`SHORTCODE_AFTER`] letters must follow it.
#[must_use]
pub fn shortcode_query(text: &str, cursor: usize) -> Option<(usize, &str)> {
    let mut cursor = cursor.min(text.len());
    while !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let before = &text[..cursor];
    let start = before
        .trim_end_matches(|c: char| !c.is_whitespace() && c != ':' && c != '@')
        .strip_suffix(':')?
        .len();
    let opens = text[..start]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace);
    let query = &before[start + 1..];
    (opens && query.chars().count() >= SHORTCODE_AFTER).then_some((start, query))
}

/// `text` with the `:shortcode` being typed at `cursor` (see [`shortcode_query`]) written as
/// `emoji` and a space, the cursor after the space; as it is when the cursor isn't in one.
#[must_use]
pub fn complete_shortcode(text: &str, cursor: usize, emoji: &str) -> (String, usize) {
    let Some((start, query)) = shortcode_query(text, cursor) else {
        return (text.to_owned(), cursor.min(text.len()));
    };
    let end = start + 1 + query.len();
    let after = &text[end..];
    let space = if after.starts_with(' ') { "" } else { " " };
    let completed = format!("{}{emoji}{space}{after}", &text[..start]);
    (completed, start + emoji.len() + 1)
}

/// `text`, typed into from `before`, with the `:shortcode:` the person just closed with the
/// colon before `cursor` (a byte offset) written as its emoji, and the cursor after it:
/// `("hi 😂", 7)` for `"hi :joy:"` at its end. `None` unless the one change from `before` is
/// that colon, so a paste, a deletion or a cursor moved is left as it is; and unless its
/// opening `:` is at the start or after a space, as [`shortcode_query`]'s is, and it's the
/// shortcode of an emoji up to `newest`.
#[must_use]
pub fn closed_shortcode(
    before: &str,
    text: &str,
    cursor: usize,
    newest: Newest,
) -> Option<(String, usize)> {
    let typed = text.get(..cursor)?.strip_suffix(':')?;
    let after = &text[cursor..];
    let only_the_colon = before.len() == typed.len() + after.len()
        && before.starts_with(typed)
        && before.ends_with(after);
    if !only_the_colon {
        return None;
    }
    let open = typed.rfind(':')?;
    let code = &typed[open + 1..];
    let opens = typed[..open]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace);
    if !opens || code.is_empty() || code.contains(char::is_whitespace) {
        return None;
    }
    let emoji = emojis::get_by_shortcode(&code.to_lowercase())
        .filter(|e| drawable(e, newest))?
        .as_str();
    Some((
        format!("{}{emoji}{after}", &typed[..open]),
        open + emoji.len(),
    ))
}

/// Puts `emoji` first in `list`, once, keeping at most [`RECENT`].
fn first(list: &mut Vec<String>, emoji: &str) {
    list.retain(|e| e != emoji);
    list.insert(0, emoji.to_owned());
    list.truncate(RECENT);
}

/// How many emoji a composer offers for a `:shortcode` at once, in every app.
pub const OFFERED: usize = 8;

/// An emoji the composer offers as someone types `:` and part of its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Suggestion {
    /// The emoji.
    pub emoji: &'static str,
    /// Its shortcode, without colons: `tada`.
    pub code: &'static str,
}

impl Suggestion {
    /// What a composer shows beside the emoji, dimmer: its shortcode as it's typed, `:tada:`.
    #[must_use]
    pub fn hint(&self) -> String {
        format!(":{}:", self.code)
    }
}

/// The emoji whose names or shortcodes `query` matches (what follows a `:` in the composer,
/// as [`sections`] finds them), at most `limit`, those up to `newest`, the ones whose shortcode
/// starts with it first.
#[must_use]
pub fn suggest(query: &str, newest: Newest, limit: usize) -> Vec<Suggestion> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let first = &words[0];
    let mut found: Vec<(bool, Suggestion)> = emojis::iter()
        .filter(|emoji| drawable(emoji, newest) && matches(emoji, &words))
        .filter_map(|emoji| {
            let code = emoji.shortcode()?;
            Some((
                !code.starts_with(first.as_str()),
                Suggestion {
                    emoji: emoji.as_str(),
                    code,
                },
            ))
        })
        .collect();
    // Stable: within each, the emoji crate's order, which is Unicode's.
    found.sort_by_key(|(later, _)| *later);
    found.into_iter().take(limit).map(|(_, s)| s).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEWEST: Newest = (15, 0);

    #[test]
    fn a_shortcode_is_a_colon_and_two_letters_at_the_start_or_after_a_space() {
        assert_eq!(shortcode_query("hi :tad", 7), Some((3, "tad")));
        assert_eq!(shortcode_query(":ta", 3), Some((0, "ta")));
        assert_eq!(shortcode_query("re:", 3), None);
        assert_eq!(shortcode_query("hi :t", 5), None, "one letter isn't enough");
        assert_eq!(shortcode_query("at 10:30", 8), None, "not a time");
        assert_eq!(shortcode_query("hello", 5), None);
        assert_eq!(
            complete_shortcode("hi :tad there", 7, "🎉"),
            ("hi 🎉 there".to_owned(), 8)
        );
        assert_eq!(
            complete_shortcode("hi :tad", 7, "🎉"),
            ("hi 🎉 ".to_owned(), 8)
        );
    }

    #[test]
    fn a_shortcode_closed_as_its_typed_becomes_its_emoji() {
        let typed =
            |before: &str, text: &str, cursor| closed_shortcode(before, text, cursor, NEWEST);
        assert_eq!(
            typed("hi :joy", "hi :joy:", 8),
            Some(("hi 😂".to_owned(), 7))
        );
        assert_eq!(typed(":+1", ":+1:", 4), Some(("👍".to_owned(), 4)));
        assert_eq!(
            typed(":tada there", ":tada: there", 6),
            Some(("🎉 there".to_owned(), 4)),
            "in the middle, what follows stays"
        );
        assert_eq!(typed(":JOY", ":JOY:", 5), Some(("😂".to_owned(), 4)));
        assert_eq!(typed("hi :nope", "hi :nope:", 9), None, "not a shortcode");
        assert_eq!(
            typed("re:joy", "re:joy:", 7),
            None,
            "opens only after a space"
        );
        assert_eq!(typed("at 10:30", "at 10:30:", 9), None);
        assert_eq!(typed("a :", "a ::", 4), None);
        assert_eq!(typed("", ":joy:", 5), None, "a paste is left as it is");
        assert_eq!(typed("hi :joy:x", "hi :joy:", 8), None, "a deletion is too");
        assert_eq!(typed("hi :joy", "hi :joy!", 8), None);
    }

    #[test]
    fn suggests_for_a_shortcode_with_its_colons_beside() {
        let found = suggest("tad", NEWEST, OFFERED);
        assert_eq!(
            found.first().map(|s| (s.emoji, s.hint())),
            Some(("🎉", ":tada:".to_owned()))
        );
    }

    #[test]
    fn writing_offers_the_emoji_used_lately_then_the_defaults() {
        let mut recent = Recent::default();
        recent.used("🐳");
        recent.reacted("🔥");
        assert_eq!(recent.to_write(4), ["🔥", "🐳", "👍", "❤️"]);
        assert_eq!(recent.quick(3), ["🔥", "👍", "❤️"]);
    }

    fn reacted(emoji: &[&str]) -> Recent {
        let mut recent = Recent::default();
        for emoji in emoji {
            recent.reacted(emoji);
        }
        recent
    }

    #[test]
    fn offers_the_usual_reactions_before_any_are_given() {
        assert_eq!(
            Recent::default().quick(QUICK),
            ["👍", "❤️", "😂", "🎉", "👀"]
        );
    }

    #[test]
    fn offers_the_reactions_given_last_first_then_the_usual_ones_once_each() {
        // 🐋 then 👍 then 🐋 again: 🐋 newest, 👍 next, then the usual ones but 👍.
        assert_eq!(
            reacted(&["🐋", "👍", "🐋"]).quick(QUICK),
            ["🐋", "👍", "❤️", "😂", "🎉"]
        );
        let many = reacted(&["1️⃣", "2️⃣", "3️⃣", "4️⃣", "5️⃣", "6️⃣"]);
        assert_eq!(many.quick(QUICK), ["6️⃣", "5️⃣", "4️⃣", "3️⃣", "2️⃣"]);
    }

    #[test]
    fn an_emoji_written_in_a_message_is_recent_but_not_a_reaction() {
        let mut recent = reacted(&["🐋"]);
        recent.used("🚀");
        assert_eq!(recent.emoji, ["🚀", "🐋"]);
        assert_eq!(recent.quick(2), ["🐋", "👍"]);
    }

    #[test]
    fn reads_what_was_kept_before_reactions_were() {
        let old: Recent = serde_json::from_str(r#"{"emoji":["🚀"]}"#).unwrap();
        assert_eq!((old.emoji.len(), old.reactions.len()), (1, 0));
    }

    #[test]
    fn finds_emoji_by_words_of_names_and_shortcodes() {
        let found = |query| match &sections(query, &[], NEWEST)[..] {
            [(Section::Results, found)] => found.clone(),
            other => panic!("{query}: {other:?}"),
        };
        assert!(found("thumbs up").contains(&"👍"));
        assert!(found("THU").contains(&"👍"));
        assert!(found("rocket").contains(&"🚀"));
        assert!(found("tada").contains(&"🎉"));
        assert!(sections("zzzzqx", &[], NEWEST).is_empty());
    }

    #[test]
    fn finds_what_is_typed_by_emoji_shortcode_or_name() {
        assert_eq!(find("🎉", NEWEST), Some("🎉"));
        assert_eq!(find("tada", NEWEST), Some("🎉"));
        assert_eq!(find(":+1:", NEWEST), Some("👍"));
        assert_eq!(find("rocket", NEWEST), Some("🚀"));
        assert_eq!(find("party popper", NEWEST), Some("🎉"));
        assert_eq!(find("zzzzqx", NEWEST), None);
        assert_eq!(find("", NEWEST), None);
    }

    #[test]
    fn lists_recent_then_every_group_in_rows() {
        let rows = rows("", &["🚀".into(), "👍".into()], NEWEST, 9);
        assert_eq!(rows[0], Row::Heading(Section::Recent));
        assert_eq!(rows[1], Row::Emoji(vec!["🚀", "👍"]));
        assert_eq!(
            rows[2],
            Row::Heading(Section::Group(Group::SmileysAndEmotion))
        );
        assert!(rows.iter().all(|row| match row {
            Row::Emoji(emoji) => !emoji.is_empty() && emoji.len() <= 9,
            Row::Heading(_) => true,
        }));
        assert!(heading_row(&rows, Section::Group(Group::Flags)).is_some());
    }

    #[test]
    fn leaves_out_emoji_newer_than_the_app_can_draw() {
        // 🫨, shaking face, is Emoji 15.0; 🥲 is 13.0.
        let all = |newest| -> Vec<&str> {
            sections("", &["🫨".into()], newest)
                .into_iter()
                .flat_map(|(_, emoji)| emoji)
                .collect()
        };
        assert!(all(NEWEST).contains(&"🫨"));
        assert!(!all((14, 0)).contains(&"🫨"));
        assert!(all((14, 0)).contains(&"🥲"));
    }

    #[test]
    fn remembers_what_was_used_lately() {
        let mut recent = Recent::default();
        for emoji in ["👍", "🚀", "👍"] {
            recent.used(emoji);
        }
        assert_eq!(recent.emoji, ["👍", "🚀"]);
        for i in 0..30 {
            recent.used(&i.to_string());
        }
        assert_eq!(recent.emoji.len(), RECENT);
    }

    #[test]
    fn names_emoji() {
        assert_eq!(name("👍"), Some("thumbs up"));
        assert_eq!(name("nope"), None);
    }

    #[test]
    fn suggests_emoji_for_what_follows_a_colon() {
        let found: Vec<&str> = suggest("tad", (15, 1), 5).iter().map(|s| s.code).collect();
        assert_eq!(found.first(), Some(&"tada"));
        assert!(suggest("", (15, 1), 5).is_empty());
        assert!(suggest("thumbs", (15, 1), 3).len() <= 3);
    }
}
