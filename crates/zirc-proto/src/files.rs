//! What sort of file an upload is, and the start of a text file to preview: the rules every
//! place that takes files goes by, said once here. zirc.dev's web app reads them through its
//! generated TypeScript ([`typescript`]) and, for what looks at bytes, through wasm
//! (`crates/zirc-media`); a self-hosted server (`crates/zirc-server`) calls them
//! directly.
//!
//! A file's kind comes from its first bytes where they say (an executable named `notes.txt`
//! is still an executable), else from its name and the type the app said. An organisation takes
//! every kind until its admins narrow it.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::media::PreviewKind;

/// The sorts of file an organisation may take or turn away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    /// Pictures, but not SVG, which can run script.
    Image,
    /// Video.
    Video,
    /// Audio.
    Audio,
    /// PDFs.
    Pdf,
    /// Text, code and Markdown, SVG included.
    Text,
    /// Office documents.
    Document,
    /// Archives.
    Archive,
    /// Programs and installers, whatever they're called.
    Executable,
    /// Anything else.
    Other,
}

/// Every kind, in the order the settings page lists them.
pub const KINDS: [FileKind; 9] = [
    FileKind::Image,
    FileKind::Video,
    FileKind::Audio,
    FileKind::Pdf,
    FileKind::Text,
    FileKind::Document,
    FileKind::Archive,
    FileKind::Executable,
    FileKind::Other,
];

impl FileKind {
    /// Its name as JSON and URLs have it, such as `pdf`.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Pdf => "pdf",
            Self::Text => "text",
            Self::Document => "document",
            Self::Archive => "archive",
            Self::Executable => "executable",
            Self::Other => "other",
        }
    }

    /// What it is, as the settings page says.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Image => "Images",
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Pdf => "PDFs",
            Self::Text => "Text, code and Markdown",
            Self::Document => "Office documents",
            Self::Archive => "Archives (zip, tar, 7z, rar)",
            Self::Executable => "Programs and installers",
            Self::Other => "Anything else",
        }
    }
}

const DOCUMENTS: &[&str] = &[
    "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "rtf", "pages", "numbers",
    "key",
];
const ARCHIVES: &[&str] = &["zip", "tar", "gz", "tgz", "bz2", "xz", "7z", "rar", "zst"];
const EXECUTABLES: &[&str] = &[
    "exe", "msi", "dmg", "pkg", "app", "apk", "ipa", "deb", "rpm", "jar", "bin", "com", "scr",
    "bat", "cmd", "ps1", "vbs",
];

/// Machine code's first bytes: Mach-O, either way round, and universal binaries.
const MACH_O: [[u8; 4]; 5] = [
    [0xfe, 0xed, 0xfa, 0xce],
    [0xfe, 0xed, 0xfa, 0xcf],
    [0xcf, 0xfa, 0xed, 0xfe],
    [0xce, 0xfa, 0xed, 0xfe],
    [0xca, 0xfe, 0xba, 0xbe],
];

/// A name's extension, lower case, or nothing.
fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.to_lowercase())
        .unwrap_or_default()
}

fn at(bytes: &[u8], offset: usize, magic: &[u8]) -> bool {
    bytes.get(offset..offset + magic.len()) == Some(magic)
}

/// Whether `bytes` read as UTF-8 text: no NULs, nothing that isn't UTF-8 (a character cut at
/// the end is fine).
#[must_use]
pub fn is_text(bytes: &[u8]) -> bool {
    if bytes.contains(&0) {
        return false;
    }
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        // Cut short at the end, rather than wrong.
        Err(e) => e.error_len().is_none(),
    }
}

/// The kind of a file named `name`, said to be `mime`, starting with `bytes`.
#[must_use]
pub fn kind_of(name: &str, mime: &str, bytes: &[u8]) -> FileKind {
    let ext = extension(name);
    let has = |list: &[&str]| list.contains(&ext.as_str());
    // Programs, whatever they're called.
    if at(bytes, 0, b"MZ") || at(bytes, 0, b"\x7fELF") || MACH_O.iter().any(|m| at(bytes, 0, m)) {
        return FileKind::Executable;
    }
    if at(bytes, 0, b"%PDF-") {
        return FileKind::Pdf;
    }
    if at(bytes, 0, b"PK\x03\x04") {
        return if has(DOCUMENTS) {
            FileKind::Document
        } else if has(EXECUTABLES) {
            FileKind::Executable
        } else {
            FileKind::Archive
        };
    }
    if at(bytes, 0, b"\xd0\xcf\x11\xe0") {
        return if has(EXECUTABLES) {
            FileKind::Executable
        } else {
            FileKind::Document
        };
    }
    if at(bytes, 0, b"\x1f\x8b")
        || at(bytes, 0, b"7z\xbc\xaf")
        || at(bytes, 0, b"Rar!")
        || at(bytes, 257, b"ustar")
        || at(bytes, 0, b"\x28\xb5\x2f\xfd")
    {
        return FileKind::Archive;
    }
    if has(EXECUTABLES) {
        return FileKind::Executable;
    }
    // SVG is a document that can run script, not a picture: text.
    if mime.starts_with("image/") && mime != "image/svg+xml" {
        return FileKind::Image;
    }
    if mime.starts_with("video/") {
        return FileKind::Video;
    }
    if mime.starts_with("audio/") {
        return FileKind::Audio;
    }
    if has(DOCUMENTS) {
        return FileKind::Document;
    }
    if has(ARCHIVES) {
        return FileKind::Archive;
    }
    let text = if bytes.is_empty() {
        mime.starts_with("text/") || mime == "image/svg+xml"
    } else {
        is_text(bytes)
    };
    if text {
        FileKind::Text
    } else {
        FileKind::Other
    }
}

/// The kind an upload says it is, before its bytes are there to look at.
#[must_use]
pub fn claimed_kind(name: &str, mime: &str) -> FileKind {
    kind_of(name, mime, &[])
}

/// How much of a file a preview shows, in bytes.
pub const PREVIEW_BYTES: usize = 50_000;

/// The languages code is highlighted as, by extension.
const LANGUAGES: &[(&str, &str)] = &[
    ("bash", "bash"),
    ("c", "c"),
    ("cc", "cpp"),
    ("cpp", "cpp"),
    ("cs", "csharp"),
    ("css", "css"),
    ("diff", "diff"),
    ("go", "go"),
    ("h", "c"),
    ("hpp", "cpp"),
    ("html", "xml"),
    ("ini", "ini"),
    ("java", "java"),
    ("js", "javascript"),
    ("json", "json"),
    ("jsx", "javascript"),
    ("kt", "kotlin"),
    ("lua", "lua"),
    ("m", "objectivec"),
    ("mjs", "javascript"),
    ("patch", "diff"),
    ("php", "php"),
    ("pl", "perl"),
    ("py", "python"),
    ("rb", "ruby"),
    ("rs", "rust"),
    ("scss", "scss"),
    ("sh", "bash"),
    ("sql", "sql"),
    ("svg", "xml"),
    ("swift", "swift"),
    ("toml", "ini"),
    ("ts", "typescript"),
    ("tsx", "typescript"),
    ("xml", "xml"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
    ("zsh", "bash"),
];

/// Whether a file named `name` is Markdown.
#[must_use]
pub fn is_markdown(name: &str) -> bool {
    matches!(extension(name).as_str(), "md" | "markdown" | "mdown")
}

/// The language a file named `name` is highlighted as, by its extension.
#[must_use]
pub fn language_for(name: &str) -> Option<&'static str> {
    let ext = extension(name);
    LANGUAGES
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, language)| *language)
}

/// What a preview shows of a text file: its start, and what to draw it as. Rendering it as
/// HTML, for a browser, is the web app's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PreviewText {
    /// Markdown or text.
    pub kind: PreviewKind,
    /// The language it's highlighted as, such as `rust`, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub language: Option<String>,
    /// Its start, up to [`PREVIEW_BYTES`], never cutting a character.
    pub text: String,
    /// Whether the file goes on past it.
    pub truncated: bool,
}

/// What a preview of a file named `name`, starting with `bytes`, UTF-8 text, shows.
#[must_use]
pub fn preview_text(name: &str, bytes: &[u8]) -> PreviewText {
    let truncated = bytes.len() > PREVIEW_BYTES;
    let start = &bytes[..bytes.len().min(PREVIEW_BYTES)];
    let text = match std::str::from_utf8(start) {
        Ok(text) => text.to_owned(),
        // A character cut at the end goes; anything else that isn't UTF-8 shows as U+FFFD.
        Err(e) if e.error_len().is_none() => {
            String::from_utf8_lossy(&start[..e.valid_up_to()]).into_owned()
        }
        Err(_) => String::from_utf8_lossy(start).into_owned(),
    };
    if is_markdown(name) {
        return PreviewText {
            kind: PreviewKind::Markdown,
            language: None,
            text,
            truncated,
        };
    }
    PreviewText {
        kind: PreviewKind::Text,
        language: language_for(name).map(str::to_owned),
        text,
        truncated,
    }
}

/// The TypeScript the web app reads these as: the kinds, their names, and how much a preview
/// shows. What looks at bytes it calls through wasm (`crates/zirc-media`).
pub(crate) fn typescript(config: &ts_rs::Config) -> Vec<String> {
    let kinds: Vec<String> = KINDS.iter().map(|k| format!("{:?}", k.key())).collect();
    let names: Vec<String> = KINDS
        .iter()
        .map(|k| format!("{}: {:?}", k.key(), k.name()))
        .collect();
    vec![
        FileKind::decl(config),
        PreviewText::decl(config),
        format!("const KINDS: readonly FileKind[] = [{}];", kinds.join(", ")),
        format!(
            "const KIND_NAMES: Record<FileKind, string> = {{ {} }};",
            names.join(", ")
        ),
        format!("const PREVIEW_BYTES = {PREVIEW_BYTES};"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(parts: &[&[u8]]) -> Vec<u8> {
        parts.concat()
    }

    #[test]
    fn kinds_come_from_the_bytes_first_whatever_the_name_says() {
        assert_eq!(
            kind_of("notes.txt", "text/plain", &[0x4d, 0x5a, 0x90, 0]),
            FileKind::Executable
        );
        assert_eq!(
            kind_of("run", "application/octet-stream", b"\x7fELF"),
            FileKind::Executable
        );
        assert_eq!(kind_of("a.txt", "text/plain", b"%PDF-1.7"), FileKind::Pdf);
        assert_eq!(
            kind_of("report.docx", "application/zip", b"PK\x03\x04"),
            FileKind::Document
        );
        assert_eq!(
            kind_of("app.apk", "application/zip", b"PK\x03\x04"),
            FileKind::Executable
        );
        assert_eq!(
            kind_of("stuff.zip", "application/zip", b"PK\x03\x04"),
            FileKind::Archive
        );
        assert_eq!(
            kind_of("x.tgz", "application/gzip", &[0x1f, 0x8b, 8]),
            FileKind::Archive
        );
    }

    #[test]
    fn then_from_the_name_and_the_type_said() {
        let mov = bytes(&[&[0, 0, 0, 0x14], b"ftypqt  "]);
        assert_eq!(
            kind_of("clip.mov", "video/quicktime", &mov),
            FileKind::Video
        );
        assert_eq!(
            kind_of("setup.exe", "application/octet-stream", b"anything"),
            FileKind::Executable
        );
        assert_eq!(kind_of("song.mp3", "audio/mpeg", b"ID3"), FileKind::Audio);
    }

    #[test]
    fn text_when_the_bytes_are_utf8_svg_included() {
        assert_eq!(
            kind_of(
                "README.md",
                "text/markdown",
                "# Hello\n\nsmörgås".as_bytes()
            ),
            FileKind::Text
        );
        assert_eq!(
            kind_of("logo.svg", "image/svg+xml", b"<svg/>"),
            FileKind::Text
        );
        assert_eq!(
            kind_of("blob", "application/octet-stream", &[0xff, 0xfe, 0, 1]),
            FileKind::Other
        );
        assert!(is_text(b"ok \xc3"));
        assert!(!is_text(&[0xc3, 0x28]));
    }

    #[test]
    fn what_an_upload_says_before_its_bytes_are_there() {
        assert_eq!(claimed_kind("a.md", "text/markdown"), FileKind::Text);
        assert_eq!(
            claimed_kind("a.dat", "application/octet-stream"),
            FileKind::Other
        );
        assert_eq!(
            claimed_kind("a.exe", "application/octet-stream"),
            FileKind::Executable
        );
    }

    #[test]
    fn a_preview_is_markdown_or_text_in_its_language() {
        assert_eq!(
            preview_text("notes.md", b"# Plan").kind,
            PreviewKind::Markdown
        );
        let code = preview_text("main.rs", b"fn main() {}");
        assert_eq!(
            (code.kind, code.language.as_deref()),
            (PreviewKind::Text, Some("rust"))
        );
        assert_eq!(preview_text("log.txt", b"x").language, None);
    }

    #[test]
    fn a_long_file_shows_its_start_cut_between_characters() {
        let long = "å".repeat(PREVIEW_BYTES);
        let preview = preview_text("log.txt", long.as_bytes());
        assert!(preview.truncated);
        assert_eq!(preview.text, "å".repeat(PREVIEW_BYTES / 2));
    }

    #[test]
    fn every_kind_has_a_key_and_a_name() {
        for kind in KINDS {
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.key());
            assert!(!kind.name().is_empty());
        }
    }
}
