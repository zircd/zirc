//! What an organisation takes as an upload, decided once: how each is stored, the largest of
//! each, which image types are resized and to which widths, and the organisation's own
//! settings (which kinds it takes, previews, how long files are kept) with why they turn one
//! away. zirc.dev runs these as wasm (`zirc-media`) and reads the constants as generated
//! TypeScript ([`typescript`]); a self-hosted server calls them as they are.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::files::{self, FileKind, KINDS};
use crate::images;
use crate::media::Purpose;

/// How an upload is stored, which decides its limit and what's made from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum Stored {
    /// A person's picture.
    Avatar,
    /// An image, resized and shown inline.
    Image,
    /// A video, played inline.
    Video,
    /// Anything else, downloaded.
    File,
}

impl Stored {
    /// Every way, in the order the limits are listed.
    pub const ALL: [Stored; 4] = [Stored::Avatar, Stored::Image, Stored::Video, Stored::File];

    /// Its key, as JSON and TypeScript say it.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Stored::Avatar => "avatar",
            Stored::Image => "image",
            Stored::Video => "video",
            Stored::File => "file",
        }
    }

    /// The largest taken, in bytes.
    #[must_use]
    pub const fn largest(self) -> u64 {
        match self {
            Stored::Avatar => 10_000_000,
            Stored::Image => 25_000_000,
            Stored::Video => 2_000_000_000,
            Stored::File => 500_000_000,
        }
    }
}

/// Image types that are resized; anything else, SVG included (it can run script), is a file.
pub const IMAGE_TYPES: [&str; 6] = [
    "image/jpeg",
    "image/png",
    "image/webp",
    "image/gif",
    "image/heic",
    "image/avif",
];

/// The media type a file's name suggests, for an upload or a link to a file: zirc.dev checks
/// an upload's bytes, so this only has to be a good guess.
#[must_use]
pub fn guess_mime(name: &str) -> &'static str {
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "heic" | "heif" => "image/heic",
        "avif" => "image/avif",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "txt" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// The widths images are resized to, smallest first. imgproxy's presets
/// (deploy/media/imgproxy.yaml) are a copy, as its config can't read this: a change goes in both.
pub const WIDTHS: [u32; 5] = [64, 256, 640, 1280, 2048];

/// What an image is served as, resized: the first unless another is asked for.
pub const FORMATS: [&str; 3] = ["webp", "avif", "jpeg"];

/// The format to serve an image as when `asked` is asked for: it, if it's one of
/// [`FORMATS`], else the first.
#[must_use]
pub fn format_for(asked: Option<&str>) -> &'static str {
    FORMATS
        .into_iter()
        .find(|&format| Some(format) == asked)
        .unwrap_or(FORMATS[0])
}

/// How long an upload's URL lasts, in seconds.
pub const UPLOAD_SECONDS: u32 = 300;

/// The random bytes a file's id is made of, as unpadded base64url: unguessable, never
/// derived from its name, and short enough for IRC.
pub const ID_BYTES: usize = 12;

/// Whether `id` could be a file's id, before it goes near a query, a key or a path.
#[must_use]
pub fn plausible_id(id: &str) -> bool {
    id.len() == ID_BYTES / 3 * 4
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The smallest of [`WIDTHS`] at least `pixels` wide, or the largest.
#[must_use]
pub fn width_for(pixels: u32) -> u32 {
    WIDTHS
        .into_iter()
        .find(|&width| width >= pixels)
        .unwrap_or(WIDTHS[WIDTHS.len() - 1])
}

/// `bytes` as people read a size: `512 B`, `14 KB`, `2.3 MB`, `1.1 GB`. zirc.dev's
/// lib/size-text.ts is a copy, as a page in the browser says sizes too: a change goes in both.
#[must_use]
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "precision isn't the point: a size shown is a size read"
    )]
    let mut size = bytes as f64 / 1000.0;
    let mut unit = 0;
    while size >= 999.95 && unit < UNITS.len() - 1 {
        size /= 1000.0;
        unit += 1;
    }
    if size < 10.0 {
        format!("{size:.1} {}", UNITS[unit])
    } else {
        format!("{size:.0} {}", UNITS[unit])
    }
}

/// How an upload for `purpose`, said to be `mime`, is stored: an image type is an image, video
/// a video, anything else a file. Video goes as a file while the organisation hasn't switched
/// it on (`video`), so it's still shared, as a download.
#[must_use]
pub fn kind_for(purpose: Purpose, mime: &str, video: bool) -> Stored {
    if matches!(purpose, Purpose::Avatar | Purpose::AccountAvatar) {
        return Stored::Avatar;
    }
    if IMAGE_TYPES.contains(&mime) {
        return Stored::Image;
    }
    let playable = ["video/mp4", "video/quicktime", "video/webm"].contains(&mime);
    if video && playable {
        Stored::Video
    } else {
        Stored::File
    }
}

/// An organisation's files settings, as its owners and admins set them on zirc.dev's Files
/// page: which kinds of file it takes, whether text files are previewed, and how long files
/// are kept. What isn't set is the default ([`Settings::default`]), so a new default reaches
/// every organisation that hasn't chosen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename = "MediaSettings")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is a switch on zirc.dev's Files page, stored as it's set"
)]
pub struct Settings {
    /// Image uploads, which are resized and shown inline.
    pub images: bool,
    /// Video uploads. Off by default: they're where storage and processing cost grows.
    pub video: bool,
    /// Files of every other kind.
    pub files: bool,
    /// Previews of text and Markdown files.
    pub previews: bool,
    /// The kinds taken: every kind unless narrowed.
    pub kinds: Vec<FileKind>,
    /// How long files are kept.
    pub retention: Retention,
    /// The largest video taken, in megabytes.
    pub video_max_mb: u32,
    /// The longest video taken, in minutes.
    pub video_max_minutes: u32,
}

/// Files older than `days` are removed, each stored kind by its own number of days where it
/// has one; none keeps them. Avatars are a person's current picture, never removed by age.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Retention {
    /// Days for every kind without its own, or `null` to keep them.
    pub days: Option<u32>,
    /// Each kind's own number of days, or `null` to keep them.
    pub kinds: RetainedKinds,
}

/// Each stored kind's own number of days, where it has one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct RetainedKinds {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Images' days.
    pub image: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Videos' days.
    pub video: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Other files' days.
    pub file: Option<u32>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            images: true,
            video: false,
            files: true,
            previews: true,
            kinds: KINDS.to_vec(),
            retention: Retention::default(),
            video_max_mb: 500,
            video_max_minutes: 10,
        }
    }
}

/// The largest video an organisation may set, in megabytes: [`Stored::Video`]'s limit.
const VIDEO_MB_MAX: u32 = 2000;

/// The longest video an organisation may set, in minutes.
const VIDEO_MINUTES_MAX: u32 = 60;

/// The most days a file may be kept for, ten years, short of keeping it.
const DAYS_MAX: u32 = 3650;

impl Settings {
    /// The settings `stored` says over the defaults, leaving out anything that isn't one: what
    /// zirc.dev keeps, or what it sends a self-hosted server.
    #[must_use]
    pub fn from_stored(stored: &Value) -> Settings {
        let defaults = Settings::default();
        let flag = |name: &str, fallback: bool| stored[name].as_bool().unwrap_or(fallback);
        let kinds = match stored["kinds"].as_array() {
            Some(given) => KINDS
                .into_iter()
                .filter(|kind| given.iter().any(|g| g.as_str() == Some(kind.key())))
                .collect(),
            None => defaults.kinds,
        };
        let retention = &stored["retention"];
        Settings {
            images: flag("images", defaults.images),
            video: flag("video", defaults.video),
            files: flag("files", defaults.files),
            previews: flag("previews", defaults.previews),
            kinds,
            retention: Retention {
                days: bounded(&retention["days"], DAYS_MAX),
                kinds: RetainedKinds {
                    image: bounded(&retention["kinds"]["image"], DAYS_MAX),
                    video: bounded(&retention["kinds"]["video"], DAYS_MAX),
                    file: bounded(&retention["kinds"]["file"], DAYS_MAX),
                },
            },
            video_max_mb: bounded(&stored["videoMaxMb"], VIDEO_MB_MAX)
                .unwrap_or(defaults.video_max_mb),
            video_max_minutes: bounded(&stored["videoMaxMinutes"], VIDEO_MINUTES_MAX)
                .unwrap_or(defaults.video_max_minutes),
        }
    }

    /// Why these settings turn away a file of `kind` stored as `stored`, said to the person
    /// uploading it, or `None` when they take it.
    #[must_use]
    pub fn refusal(&self, kind: FileKind, stored: Stored) -> Option<String> {
        if stored == Stored::Image && !self.images {
            return Some("This organisation doesn't take images.".into());
        }
        if (stored == Stored::Video || kind == FileKind::Video) && !self.video {
            return Some("This organisation doesn't take video.".into());
        }
        if stored == Stored::File && !self.files {
            return Some("This organisation takes only images and video.".into());
        }
        if stored != Stored::Avatar && !self.kinds.contains(&kind) {
            return Some(format!("This organisation doesn't take {}.", words(kind)));
        }
        None
    }
}

/// A whole number from 1 to `max`, or `None`.
fn bounded(value: &Value, max: u32) -> Option<u32> {
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| (1..=max).contains(n))
}

/// Files of `kind`, as a refusal says them.
const fn words(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Image => "images",
        FileKind::Video => "video",
        FileKind::Audio => "audio",
        FileKind::Pdf => "PDFs",
        FileKind::Text => "text files",
        FileKind::Document => "office documents",
        FileKind::Archive => "archives",
        FileKind::Executable => "programs or installers",
        FileKind::Other => "files of that kind",
    }
}

/// What an upload is to be, once it's said what it is: how it's stored, and its name and type
/// made safe to keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Starting {
    /// How it's stored.
    pub stored: Stored,
    /// Its name, without control characters or slashes, at most 200 characters.
    pub name: String,
    /// Its type in lowercase, or `application/octet-stream` when it isn't one.
    pub mime: String,
}

/// What an upload for `purpose`, named `name`, said to be `mime`, of `size` bytes, is to be
/// under `settings`, before its bytes are sent; [`settle`] checks again from them.
///
/// # Errors
///
/// A [`Refused`]: a picture that isn't an image type (415), a kind the organisation doesn't
/// take (403), no size (400), or more than its kind's or the organisation's largest (413).
pub fn start(
    settings: &Settings,
    purpose: Purpose,
    name: &str,
    mime: &str,
    size: u64,
) -> Result<Starting, Refused> {
    let refused = |status, why: String| Err(Refused { status, why });
    let stored = kind_for(purpose, mime, settings.video);
    if stored == Stored::Avatar && !IMAGE_TYPES.contains(&mime) {
        return refused(
            415,
            "A picture has to be a JPEG, PNG, WebP, GIF, HEIC or AVIF image.".into(),
        );
    }
    // As the app says it is; settle checks again, from its bytes.
    if let Some(why) = settings.refusal(files::kind_of(name, mime, &[]), stored) {
        return refused(403, why);
    }
    if size == 0 {
        return refused(400, "Say the file's size in bytes.".into());
    }
    if size > stored.largest() {
        let what = if stored == Stored::File {
            "file"
        } else {
            stored.key()
        };
        return refused(
            413,
            format!(
                "That's {}: the largest {what} is {}.",
                size_text(size),
                size_text(stored.largest())
            ),
        );
    }
    if stored == Stored::Video && size > u64::from(settings.video_max_mb) * 1_000_000 {
        return refused(
            413,
            format!(
                "That's {}: the largest video here is {} MB.",
                size_text(size),
                settings.video_max_mb
            ),
        );
    }
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_control() || c == '/' || c == '\\' {
                '_'
            } else {
                c
            }
        })
        .take(200)
        .collect();
    let typed = mime.split_once('/').is_some_and(|(top, sub)| {
        !top.is_empty()
            && top.bytes().all(|b| b.is_ascii_alphabetic())
            && !sub.is_empty()
            && sub
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".+-".contains(&b))
    });
    Ok(Starting {
        stored,
        name: if name.is_empty() { "file".into() } else { name },
        mime: if typed {
            mime.to_ascii_lowercase()
        } else {
            "application/octet-stream".into()
        },
    })
}

/// Why an upload whose bytes came to `sent` when it said `said` is turned away (400), or
/// `None` when they're the same.
#[must_use]
pub fn wrong_size(sent: u64, said: u64) -> Option<Refused> {
    (sent != said).then(|| Refused {
        status: 400,
        why: format!(
            "The file uploaded was {}, not the {} it said.",
            size_text(sent),
            size_text(said)
        ),
    })
}

/// What arrived for an upload, as its server read it once its bytes were in: what it was to
/// be stored as, and said to be, and its first bytes (at least [`files::PREVIEW_BYTES`] and
/// one more, when it has them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arrived<'a> {
    /// How it was to be stored, from [`kind_for`] when it started.
    pub stored: Stored,
    /// Its name as uploaded.
    pub name: &'a str,
    /// The type the app said it is.
    pub mime: &'a str,
    /// Its first bytes.
    pub start: &'a [u8],
    /// A video's size and length, as the media worker read them, or `None` when it couldn't.
    pub video: Option<Video>,
}

/// A video's size and length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Video {
    /// Its width in pixels.
    pub width: u32,
    /// Its height in pixels.
    pub height: u32,
    /// Its length in milliseconds.
    pub duration_ms: u64,
}

/// What an upload is, once its bytes are in: how it's kept, its type from its bytes, its size,
/// and whether it's text, which is previewed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct Settled {
    /// How it's kept, which may not be what it was to be: an image that isn't one is a file.
    pub stored: Stored,
    /// Its type as it's served.
    pub mime: String,
    /// An image's or video's width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// An image's or video's height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// A video's length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Whether it's text or Markdown, which apps draw as text and may be previewed.
    pub text: bool,
}

/// Why an upload is turned away once its bytes are in, and the HTTP status that says so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Refused {
    /// 403, 413 or 415.
    pub status: u16,
    /// What the person is told.
    pub why: String,
}

/// The video types played as they are; any other video is served as MP4.
const PLAYABLE: [&str; 3] = ["video/mp4", "video/quicktime", "video/webm"];

/// What `arrived` is under `settings`, or why it's turned away: its type from its bytes, an
/// image's size upright, and an image that isn't one or a video the worker couldn't read kept
/// as a file, served only as a download.
///
/// # Errors
///
/// A [`Refused`]: an avatar that isn't an image (415), a video longer than the organisation
/// takes (413), or a kind it doesn't take (403).
pub fn settle(settings: &Settings, arrived: &Arrived) -> Result<Settled, Refused> {
    let refused = |status, why: String| Err(Refused { status, why });
    let mut stored = arrived.stored;
    let mut mime = arrived.mime.to_owned();
    let mut size = None;
    if matches!(stored, Stored::Avatar | Stored::Image) {
        match images::probe(arrived.start) {
            Some(found) => {
                size = Some((found.width, found.height));
                mime = found.mime;
            }
            None if stored == Stored::Avatar => {
                return refused(415, "That isn't an image Zirc can show.".into());
            }
            None => {
                stored = Stored::File;
                "application/octet-stream".clone_into(&mut mime);
            }
        }
    }
    let mut duration_ms = None;
    if stored == Stored::Video {
        match arrived.video {
            None => {
                stored = Stored::File;
                "application/octet-stream".clone_into(&mut mime);
            }
            Some(video) if video.duration_ms > u64::from(settings.video_max_minutes) * 60_000 => {
                let minutes = video.duration_ms.div_ceil(60_000);
                return refused(
                    413,
                    format!(
                        "That's {minutes} minutes long: the longest video here is {}.",
                        settings.video_max_minutes
                    ),
                );
            }
            Some(video) => {
                size = Some((video.width, video.height));
                duration_ms = Some(video.duration_ms);
                if !PLAYABLE.contains(&mime.as_str()) {
                    "video/mp4".clone_into(&mut mime);
                }
            }
        }
    }
    let kind = match stored {
        Stored::File => files::kind_of(arrived.name, arrived.mime, arrived.start),
        Stored::Avatar | Stored::Image => FileKind::Image,
        Stored::Video => FileKind::Video,
    };
    if let Some(why) = settings.refusal(kind, stored) {
        return refused(403, why);
    }
    // Only text is said to be text, so apps draw it as text.
    let text = stored == Stored::File && kind == FileKind::Text;
    if text {
        mime = if files::is_markdown(arrived.name) {
            "text/markdown"
        } else {
            "text/plain"
        }
        .into();
    } else if stored == Stored::File && mime.starts_with("text/") {
        "application/octet-stream".clone_into(&mut mime);
    }
    let size = size.filter(|_| stored != Stored::File);
    Ok(Settled {
        stored,
        mime,
        width: size.map(|(w, _)| w),
        height: size.map(|(_, h)| h),
        duration_ms,
        text,
    })
}

/// The types and constants above, for zirc.dev's TypeScript.
pub(crate) fn typescript(config: &ts_rs::Config) -> Vec<String> {
    let quoted = |items: &[&str]| {
        items
            .iter()
            .map(|item| format!("{item:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let limits: Vec<String> = Stored::ALL
        .iter()
        .map(|s| format!("{}: {}", s.key(), s.largest()))
        .collect();
    let widths: Vec<String> = WIDTHS.iter().map(ToString::to_string).collect();
    let defaults = serde_json::to_string(&Settings::default()).unwrap_or_default();
    vec![
        Stored::decl(config),
        Video::decl(config),
        Settled::decl(config),
        Refused::decl(config),
        images::Probed::decl(config),
        RetainedKinds::decl(config),
        Retention::decl(config),
        Settings::decl(config),
        format!(
            "const LIMITS: Record<Stored, number> = {{ {} }};",
            limits.join(", ")
        ),
        format!("const IMAGE_TYPES = [{}] as const;", quoted(&IMAGE_TYPES)),
        format!("const WIDTHS = [{}] as const;", widths.join(", ")),
        format!("const FORMATS = [{}] as const;", quoted(&FORMATS)),
        format!("const UPLOAD_SECONDS = {UPLOAD_SECONDS};"),
        format!("const ID_BYTES = {ID_BYTES};"),
        format!("const MEDIA_DEFAULTS: MediaSettings = {defaults};"),
    ]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn what_is_left_out_is_the_default_and_what_is_not_a_setting_is_dropped() {
        assert_eq!(Settings::from_stored(&Value::Null), Settings::default());
        let set = Settings::from_stored(&json!({
            "video": true, "kinds": ["pdf", "nonsense", "image"], "videoMaxMb": 99999,
            "videoMaxMinutes": 30, "retention": {"days": 0, "kinds": {"image": 2.5, "video": 9999, "file": 30}},
        }));
        assert!(set.video && set.images);
        // In the settings page's order, not as sent.
        assert_eq!(set.kinds, [FileKind::Image, FileKind::Pdf]);
        assert_eq!(set.video_max_mb, 500);
        assert_eq!(set.video_max_minutes, 30);
        assert_eq!(set.retention.days, None);
        assert_eq!(
            set.retention.kinds,
            RetainedKinds {
                file: Some(30),
                ..Default::default()
            }
        );
    }

    #[test]
    fn refusals_say_what_the_organisation_turns_away() {
        let mut settings = Settings::default();
        assert_eq!(
            settings.refusal(FileKind::Video, Stored::File).as_deref(),
            Some("This organisation doesn't take video.")
        );
        settings.video = true;
        assert_eq!(settings.refusal(FileKind::Video, Stored::Video), None);
        settings.kinds.retain(|k| *k != FileKind::Executable);
        assert_eq!(
            settings
                .refusal(FileKind::Executable, Stored::File)
                .as_deref(),
            Some("This organisation doesn't take programs or installers.")
        );
        // A person's picture is never turned away by the kinds.
        settings.kinds.clear();
        assert_eq!(settings.refusal(FileKind::Image, Stored::Avatar), None);
    }

    #[test]
    fn uploads_are_stored_by_purpose_and_type() {
        assert_eq!(kind_for(Purpose::Share, "image/heic", false), Stored::Image);
        assert_eq!(
            kind_for(Purpose::Share, "image/svg+xml", false),
            Stored::File
        );
        assert_eq!(kind_for(Purpose::Share, "video/mp4", false), Stored::File);
        assert_eq!(kind_for(Purpose::Share, "video/mp4", true), Stored::Video);
        assert_eq!(
            kind_for(Purpose::Avatar, "image/png", false),
            Stored::Avatar
        );
    }

    fn arrived<'a>(stored: Stored, name: &'a str, mime: &'a str, start: &'a [u8]) -> Arrived<'a> {
        Arrived {
            stored,
            name,
            mime,
            start,
            video: None,
        }
    }

    #[test]
    fn an_upload_is_what_its_bytes_say() {
        let settings = Settings::default();
        let gif = b"GIF89a\x40\x01\xf0\x00";
        let image = settle(
            &settings,
            &arrived(Stored::Image, "a.png", "image/png", gif),
        )
        .unwrap();
        assert_eq!(
            (image.mime.as_str(), image.width, image.height),
            ("image/gif", Some(320), Some(240))
        );
        // An image that isn't one is kept as a file, without a size; an avatar is turned away.
        let fake = settle(
            &settings,
            &arrived(Stored::Image, "a.png", "image/png", b"%PDF-1.7"),
        )
        .unwrap();
        assert_eq!(
            (fake.stored, fake.mime.as_str(), fake.width),
            (Stored::File, "application/octet-stream", None)
        );
        assert_eq!(
            settle(
                &settings,
                &arrived(Stored::Avatar, "a.png", "image/png", b"x")
            )
            .unwrap_err()
            .status,
            415
        );
        // Only text is said to be text.
        let notes = settle(
            &settings,
            &arrived(Stored::File, "notes.md", "text/plain", b"# hi"),
        )
        .unwrap();
        assert_eq!((notes.mime.as_str(), notes.text), ("text/markdown", true));
        let page = settle(
            &settings,
            &arrived(Stored::File, "a.html", "text/html", b"\0\x01"),
        )
        .unwrap();
        assert_eq!(
            (page.mime.as_str(), page.text),
            ("application/octet-stream", false)
        );
    }

    #[test]
    fn a_video_is_checked_against_the_organisation_s_length() {
        let settings = Settings {
            video: true,
            ..Settings::default()
        };
        let video = |duration_ms| Arrived {
            video: Some(Video {
                width: 1920,
                height: 1080,
                duration_ms,
            }),
            ..arrived(Stored::Video, "a.mov", "video/x-matroska", b"")
        };
        let ok = settle(&settings, &video(60_000)).unwrap();
        assert_eq!(
            (ok.mime.as_str(), ok.duration_ms, ok.width),
            ("video/mp4", Some(60_000), Some(1920))
        );
        let long = settle(&settings, &video(10 * 60_000 + 1)).unwrap_err();
        assert_eq!(
            (long.status, long.why.as_str()),
            (413, "That's 11 minutes long: the longest video here is 10.")
        );
        let unread = settle(
            &settings,
            &arrived(Stored::Video, "a.mp4", "video/mp4", b""),
        )
        .unwrap();
        assert_eq!(unread.stored, Stored::File);
        // Turned away by kind, once it's known.
        let none = Settings {
            kinds: vec![],
            ..Settings::default()
        };
        assert_eq!(
            settle(&none, &arrived(Stored::File, "a.txt", "text/plain", b"hi"))
                .unwrap_err()
                .status,
            403
        );
    }

    #[test]
    fn an_upload_starts_as_what_it_says_within_its_limits() {
        let settings = Settings::default();
        let ok = start(&settings, Purpose::Share, "a/b\nc.png", "image/png", 10).unwrap();
        assert_eq!(
            (ok.stored, ok.name.as_str(), ok.mime.as_str()),
            (Stored::Image, "a_b_c.png", "image/png")
        );
        assert_eq!(
            start(&settings, Purpose::Share, "a.txt", "Text/Plain", 1)
                .unwrap()
                .mime,
            "text/plain"
        );
        assert_eq!(
            start(&settings, Purpose::Share, "", "nonsense", 1)
                .unwrap()
                .mime,
            "application/octet-stream"
        );
        assert_eq!(
            start(&settings, Purpose::Share, "", "", 1).unwrap().name,
            "file"
        );
        assert_eq!(
            start(&settings, Purpose::Avatar, "a.svg", "image/svg+xml", 1)
                .unwrap_err()
                .status,
            415
        );
        assert_eq!(
            start(&settings, Purpose::Share, "a.txt", "text/plain", 0)
                .unwrap_err()
                .status,
            400
        );
        let large = start(
            &settings,
            Purpose::Share,
            "a.zip",
            "application/zip",
            500_000_001,
        )
        .unwrap_err();
        assert_eq!(large.why, "That's 500 MB: the largest file is 500 MB.");
        let video = Settings {
            video: true,
            video_max_mb: 5,
            ..Settings::default()
        };
        let long = start(&video, Purpose::Share, "a.mp4", "video/mp4", 6_000_000).unwrap_err();
        assert_eq!(long.why, "That's 6.0 MB: the largest video here is 5 MB.");
    }

    #[test]
    fn ids_and_widths() {
        assert!(plausible_id("abcdefghijklmn-_"));
        assert!(!plausible_id("abcdefghijklmn/_"));
        assert!(!plausible_id("short"));
        assert_eq!(width_for(1), 64);
        assert_eq!(width_for(641), 1280);
        assert_eq!(width_for(9000), 2048);
    }
}
