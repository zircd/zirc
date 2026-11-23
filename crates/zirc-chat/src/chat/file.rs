//! Files shared in chat, as every app draws them: what shape each is, how big it reads, and
//! which of zirc.dev's sizes to ask for so an image comes no larger than it's drawn.

use zirc_proto::File;

/// The widths images are resized to, smallest first, and the one to ask for.
pub use zirc_proto::uploads::{WIDTHS, size_text, width_for};

/// How many lines of a text file its card shows, in every app, as Slack's snippet does.
pub const PREVIEW_LINES: usize = 8;

/// What a text file's card shows after its lines when there's more: `…`.
pub const PREVIEW_MORE: &str = "…";

/// What an image pasted into the message field is called where it's shared, in every app:
/// `image.png`, by its format's `extension`, as a pasted image has no name of its own.
#[must_use]
pub fn pasted_name(extension: &str) -> String {
    format!("image.{}", extension.trim_start_matches('.').to_lowercase())
}

/// What a text file's card shows of `text`, the start zirc.dev previewed (`truncated` when it
/// cut it short itself): its first [`PREVIEW_LINES`] lines, blank lines at its end dropped,
/// and whether [`PREVIEW_MORE`] follows them. The one rule every app draws.
#[must_use]
pub fn preview_lines(text: &str, truncated: bool) -> (Vec<String>, bool) {
    let mut lines: Vec<&str> = text.lines().collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    let more = truncated || lines.len() > PREVIEW_LINES;
    let shown = lines
        .into_iter()
        .take(PREVIEW_LINES)
        .map(str::to_owned)
        .collect();
    (shown, more)
}

/// [`preview_lines`] as one text, as the phones draw it: its lines, then [`PREVIEW_MORE`] on
/// a line of its own when there's more.
#[must_use]
pub fn preview_text(text: &str, truncated: bool) -> String {
    let (lines, more) = preview_lines(text, truncated);
    let mut shown = lines.join("\n");
    if more {
        shown.push('\n');
        shown.push_str(PREVIEW_MORE);
    }
    shown
}

/// How a file is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Inline, resized by zirc.dev.
    Image,
    /// A poster and a player.
    Video,
    /// A card with the start of its text, which zirc.dev previews.
    Text,
    /// A card with its name, size and a download button.
    Other,
}

/// How `file` is drawn, by its media type.
#[must_use]
pub fn shape(file: &File) -> Shape {
    shape_of(&file.mime)
}

/// How a link to a file at `url` is drawn where only its address is known, as a viewer opens
/// a link a message holds: by the media type its name suggests
/// ([`zirc_proto::uploads::guess_mime`]), [`Shape::Other`] for a name that says none.
#[must_use]
pub fn link_shape(url: &str) -> Shape {
    let path = url.split(['?', '#']).next().unwrap_or_default();
    shape_of(zirc_proto::uploads::guess_mime(
        path.rsplit('/').next().unwrap_or_default(),
    ))
}

/// How a file of the media type `mime` is drawn.
fn shape_of(mime: &str) -> Shape {
    match mime.split('/').next() {
        Some("image") => Shape::Image,
        Some("video") => Shape::Video,
        Some("text") => Shape::Text,
        _ => Shape::Other,
    }
}

/// Whether `file` is a GIF, which may move: every app plays one
/// [`super::ANIMATION_PLAYS`] times, then rests it on its last frame.
#[must_use]
pub fn animated(file: &File) -> bool {
    file.mime.eq_ignore_ascii_case("image/gif")
}

/// `file`'s id at zirc.dev: the last part of its link.
#[must_use]
pub fn id(file: &File) -> &str {
    let path = file.url.split(['?', '#']).next().unwrap_or_default();
    path.rsplit('/').next().unwrap_or_default()
}

/// Whether `file` is hosted by the web app at `web`, such as `https://zirc.dev`: only then
/// may an app load it, link to it, or send it anything. A file's url is whatever its sender
/// wrote, so one elsewhere could see who looks at it, or worse.
#[must_use]
pub fn is_hosted(file: &File, web: &str) -> bool {
    // A file's own link and nothing else of the web app's: `/m/<organisation>/<id>`, or
    // `/m/<id>` on a server that hosts its own, with no query for the sender to steer what
    // comes back (the original where a preview was asked for) and no other page of the site
    // for an app to ask for as its person.
    let part = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    };
    let Some(rest) = file
        .url
        .strip_prefix(&format!("{}/m/", web.trim_end_matches('/')))
    else {
        return false;
    };
    match rest.split_once('/') {
        Some((organisation, id)) => part(organisation) && part(id),
        None => part(rest),
    }
}

/// Where to get `file`'s image, or a video's poster, to draw it `pixels` wide: device
/// pixels, so a view doubles its points on a retina screen.
#[must_use]
pub fn image_url(file: &File, pixels: u32) -> String {
    format!("{}?w={}", file.url, width_for(pixels))
}

/// The size to draw an image of `width` by `height` at, no wider than `max_width`, no
/// taller than `max_height` and never larger than it is: `None` when its size isn't known.
#[must_use]
pub fn fit(
    width: Option<u32>,
    height: Option<u32>,
    max_width: u32,
    max_height: u32,
) -> Option<(u32, u32)> {
    let (width, height) = (width?, height?);
    if width == 0 || height == 0 {
        return None;
    }
    let scale = f64::from(max_width) / f64::from(width);
    let scale = scale
        .min(f64::from(max_height) / f64::from(height))
        .min(1.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "both are at most the maxima, which are u32s"
    )]
    let scaled = |side: u32| ((f64::from(side) * scale).round() as u32).max(1);
    Some((scaled(width), scaled(height)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_files_card_shows_its_first_lines_and_says_when_there_is_more() {
        let ten = (1..=10)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (lines, more) = preview_lines(&ten, false);
        assert_eq!((lines.len(), more), (PREVIEW_LINES, true));
        // Blank lines at its end aren't more.
        assert_eq!(
            preview_lines("a\nb\n\n  \n", false),
            (vec!["a".into(), "b".into()], false)
        );
        assert_eq!(preview_text("a\nb", true), format!("a\nb\n{PREVIEW_MORE}"));
        assert_eq!(preview_text("a", false), "a");
    }

    fn file(mime: &str) -> File {
        File {
            url: "https://zirc.dev/m/acme/abc".into(),
            name: "a".into(),
            mime: mime.into(),
            size: 1,
            width: None,
            height: None,
            duration_ms: None,
        }
    }

    #[test]
    fn shapes_come_from_the_media_type() {
        assert_eq!(shape(&file("image/webp")), Shape::Image);
        assert_eq!(shape(&file("video/mp4")), Shape::Video);
        assert_eq!(shape(&file("text/markdown")), Shape::Text);
        assert_eq!(shape(&file("application/pdf")), Shape::Other);
        assert_eq!(shape(&file("")), Shape::Other);
    }

    #[test]
    fn a_pasted_image_is_named_by_its_format() {
        assert_eq!(pasted_name("png"), "image.png");
        assert_eq!(pasted_name(".JPEG"), "image.jpeg");
    }

    #[test]
    fn only_a_gif_animates() {
        assert!(animated(&file("image/gif")));
        assert!(!animated(&file("image/png")));
        assert!(!animated(&file("video/mp4")));
    }

    #[test]
    fn a_links_shape_comes_from_its_name() {
        assert_eq!(
            link_shape("https://zirc.dev/m/acme/demo.MP4?w=640"),
            Shape::Video
        );
        assert_eq!(
            link_shape("https://example.com/clip.webm#t=3"),
            Shape::Video
        );
        assert_eq!(link_shape("https://zirc.dev/m/acme/cat.gif"), Shape::Image);
        // A hosted file's own link says nothing of what it is.
        assert_eq!(link_shape("https://zirc.dev/m/acme/abc123"), Shape::Other);
    }

    #[test]
    fn ids_are_the_end_of_the_link() {
        assert_eq!(id(&file("text/plain")), "abc");
    }

    #[test]
    fn sizes_read_as_people_say_them() {
        assert_eq!(size_text(512), "512 B");
        assert_eq!(size_text(14_200), "14 KB");
        assert_eq!(size_text(2_345_678), "2.3 MB");
        assert_eq!(size_text(999_999), "1.0 MB");
        assert_eq!(size_text(1_100_000_000), "1.1 GB");
    }

    #[test]
    fn images_are_asked_for_at_the_next_size_up() {
        assert_eq!(width_for(1), 64);
        assert_eq!(width_for(64), 64);
        assert_eq!(width_for(65), 256);
        assert_eq!(width_for(5000), 2048);
        assert_eq!(
            image_url(&file("image/png"), 600),
            "https://zirc.dev/m/acme/abc?w=640"
        );
    }

    #[test]
    fn images_fit_their_box_and_never_grow() {
        assert_eq!(fit(Some(4032), Some(3024), 400, 300), Some((400, 300)));
        assert_eq!(fit(Some(1000), Some(4000), 400, 300), Some((75, 300)));
        assert_eq!(fit(Some(100), Some(50), 400, 300), Some((100, 50)));
        assert_eq!(fit(None, Some(50), 400, 300), None);
        assert_eq!(fit(Some(0), Some(50), 400, 300), None);
    }

    #[test]
    fn hosted_only_under_the_web_app() {
        let mut hosted = file("image/png");
        assert!(is_hosted(&hosted, "https://zirc.dev"));
        assert!(is_hosted(&hosted, "https://zirc.dev/"));
        hosted.url = "https://zirc.dev.evil.example/m/acme/abc".into();
        assert!(!is_hosted(&hosted, "https://zirc.dev"));
        hosted.url = "https://evil.example/m/acme/abc".into();
        assert!(!is_hosted(&hosted, "https://zirc.dev"));
        // The site's other pages aren't files, nor is a file's link with more said after it.
        for other in [
            "https://zirc.dev/api/me",
            "https://zirc.dev/m/acme/abc?original&x=",
            "https://zirc.dev/m/acme/abc/../../orgs",
            "https://zirc.dev/m/acme/abc#x",
            "https://zirc.dev/m/",
        ] {
            hosted.url = other.into();
            assert!(!is_hosted(&hosted, "https://zirc.dev"), "{other}");
        }
        hosted.url = "https://chat.acme.example/m/7Hq2xV9k".into();
        assert!(is_hosted(&hosted, "https://chat.acme.example"));
    }
}
