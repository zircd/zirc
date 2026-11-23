//! Files attached to a person's message: the words and the files in one message (the user's
//! choice, 23 Nov 2026). Until then a file was always a message of its own, [`Event::File`],
//! uploaded and sent the moment it was chosen, with no words beside it.
//!
//! The message stays a person's own words, a plain `PRIVMSG`, so everything that works on
//! words still does: mentions, pushes, search, replies, editing and sealed parts. Its files
//! ride in a client tag, [`TAG`], as a sealed message's parts do. Its text ends with the
//! files' links ([`text`]), so a plain IRC client and a zirc that predates this read the
//! words and can open the files; a zirc that knows the tag shows the words alone
//! ([`words`]) and draws the files. A file with no words beside it still goes as
//! [`Event::File`], which every zirc already draws.
//!
//! Declared here once: zirc-chat sends and reads it for every app.
//!
//! [`Event::File`]: crate::Event::File

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::File;

/// The client tag a message's files go in. Its value is [`value`].
pub const TAG: &str = "+zirc/files";

/// The most files one message carries: what fits its client tags beside a reply's id and
/// sealed parts, with room to spare, and as many as a row of thumbnails shows at once.
pub const MAX_FILES: usize = 6;

/// [`TAG`]'s value for `files`: their JSON, in base64 so it needs no escaping in a tag.
#[must_use]
pub fn value(files: &[File]) -> String {
    BASE64.encode(serde_json::to_vec(files).unwrap_or_default())
}

/// The files a [`TAG`] value names: none when it can't be read, is empty, or names more
/// than [`MAX_FILES`], which no zirc sends.
#[must_use]
pub fn parse(value: &str) -> Vec<File> {
    BASE64
        .decode(value)
        .ok()
        .and_then(|json| serde_json::from_slice::<Vec<File>>(&json).ok())
        .filter(|files| files.len() <= MAX_FILES)
        .unwrap_or_default()
}

/// The text a message with `files` is sent as: its `words`, then each file's link, with a
/// space between, which is what a reader that doesn't know [`TAG`] shows.
#[must_use]
pub fn text(words: &str, files: &[File]) -> String {
    let words = words.trim_end();
    let links = files.iter().map(|file| file.url.as_str());
    std::iter::once(words)
        .filter(|words| !words.is_empty())
        .chain(links)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The words of a message whose text is `text` and whose tag names `files`: the text
/// without the files' links it ends with ([`text`]). Text that doesn't end with them, as
/// one edited by a zirc that predates the tag, is left whole.
#[must_use]
pub fn words<'a>(text: &'a str, files: &[File]) -> &'a str {
    let mut rest = text;
    for file in files.iter().rev() {
        let Some(before) = rest.trim_end().strip_suffix(file.url.as_str()) else {
            return text;
        };
        rest = before;
    }
    rest.trim_end()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str) -> File {
        File {
            url: format!("https://zirc.dev/m/acme/{name}"),
            name: format!("{name}.png"),
            mime: "image/png".into(),
            size: 2048,
            width: Some(640),
            height: Some(480),
            duration_ms: None,
        }
    }

    #[test]
    fn a_tags_value_reads_back_as_its_files() {
        let files = [file("a"), file("b")];
        assert_eq!(parse(&value(&files)), files);
        assert!(parse("not base64 json").is_empty());
        let many: Vec<File> = (0..=MAX_FILES).map(|n| file(&n.to_string())).collect();
        assert!(parse(&value(&many)).is_empty());
    }

    #[test]
    fn the_text_is_the_words_then_the_links_and_the_words_come_back() {
        let files = [file("a"), file("b")];
        let sent = text("the before and after ", &files);
        assert_eq!(
            sent,
            "the before and after https://zirc.dev/m/acme/a https://zirc.dev/m/acme/b"
        );
        assert_eq!(words(&sent, &files), "the before and after");
        // Files alone: their links, and no words.
        assert_eq!(
            text("", &files),
            "https://zirc.dev/m/acme/a https://zirc.dev/m/acme/b"
        );
        assert_eq!(words(&text("", &files), &files), "");
    }

    #[test]
    fn text_that_does_not_end_with_the_links_is_left_whole() {
        let files = [file("a")];
        assert_eq!(words("edited elsewhere", &files), "edited elsewhere");
        assert_eq!(
            words("https://zirc.dev/m/acme/a and more", &files),
            "https://zirc.dev/m/acme/a and more"
        );
    }

    #[test]
    fn the_most_files_fit_a_messages_tags() {
        let most: Vec<File> = (0..MAX_FILES)
            .map(|n| file(&format!("0123456789abcdef{n}")))
            .collect();
        assert!(TAG.len() + 1 + value(&most).len() < crate::sealed::CLIENT_TAGS_MAX / 2);
    }
}
