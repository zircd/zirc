//! Files at zirc.dev: how a zirc app uploads one to share in chat or as a person's avatar,
//! and removes it. The bytes go straight to storage; the web app checks and records them:
//!
//! 1. `POST /api/media` with an [`UploadRequest`] answers an [`UploadStarted`]: the file's id
//!    and a signed URL valid for a few minutes, for exactly that many bytes of that type.
//!    It answers 403 when the organisation has that kind of file switched off, and 413 when
//!    the file is larger than its limit or would take the organisation over its quota, with
//!    a JSON `error` saying which.
//! 2. `PUT` the bytes to that URL, with [`UploadStarted::headers`].
//! 3. `POST /api/media/<id>/done` answers the [`crate::File`] to share, once zirc.dev has
//!    checked its type from its bytes and read an image's size. The original is kept as
//!    sent; what zirc.dev serves from it, resized, has no metadata, GPS included. 415 when
//!    its bytes aren't what it said, or can't be taken.
//!
//! `GET /api/media/<id>/preview` answers a text or Markdown file's [`Preview`], made on
//! zirc.dev from its start: 404 for a file that has none.
//!
//! `DELETE /api/media/<id>` removes a file and everything made from it, for its uploader and
//! the organisation's admins: 204, or 404.
//!
//! Each is authenticated as the app's account with HTTP Basic, as [`crate::login`] says,
//! or, in a browser on zirc.dev, by its session. Files are downloaded from their
//! [`crate::File::url`] by a member of the organisation, signed in or as an app's account,
//! which redirects to where the bytes are for a few minutes; an image at `?w=<width>`.
//! A member's avatar is at `GET /api/media/avatar/<nick>?w=<width>` for an app's account,
//! the nick a person's or a machine's (which shows its owner's), the organisation the
//! account's: 404 when they have none, and the app shows their initials.
//!
//! The web app's TypeScript types are generated from these with [`crate::login`]'s.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where zirc.dev sends a self-hosted server the organisation's files settings
/// (`crate::uploads::Settings`), as JSON, under its [`crate::join::SIDECAR_PATH`], with the
/// accounts token: the server answers it itself.
pub const SETTINGS_PATH: &str = "/v1/media/settings";

/// Where an upload starts, under the web app's address.
pub const UPLOAD_PATH: &str = "/api/media";

/// Where an upload is said to be done, under the web app's address.
#[must_use]
pub fn done_path(id: &str) -> String {
    format!("{UPLOAD_PATH}/{id}/done")
}

/// Where a text or Markdown file's preview is, under the web app's address.
#[must_use]
pub fn preview_path(id: &str) -> String {
    format!("{UPLOAD_PATH}/{id}/preview")
}

/// What sort of preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum PreviewKind {
    /// Text or code, highlighted for its language.
    Text,
    /// Markdown, rendered.
    Markdown,
}

/// The start of a text or Markdown file, as zirc.dev shows it: its source for the apps to
/// draw as styled text, and HTML, sanitised, for a browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct Preview {
    /// What sort.
    pub kind: PreviewKind,
    /// The language it's highlighted as, such as `rust`, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Its start, as written.
    pub text: String,
    /// Its start rendered: highlighted code, or Markdown with no scripts, raw HTML, forms or
    /// outside images.
    pub html: String,
    /// Whether there's more than [`Preview::text`] holds.
    pub truncated: bool,
}

/// Where a file is removed, under the web app's address.
#[must_use]
pub fn file_path(id: &str) -> String {
    format!("{UPLOAD_PATH}/{id}")
}

/// Where an app gets the avatar of `nick`, a person or a machine, in its account's
/// organisation, under the web app's address.
#[must_use]
pub fn avatar_path(nick: &str) -> String {
    format!("{UPLOAD_PATH}/avatar/{nick}")
}

/// What a file is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum Purpose {
    /// To share in chat.
    Share,
    /// The person's picture in the organisation alone, replacing the one before there.
    Avatar,
    /// The picture of the person's zirc.dev account, which every organisation they're in
    /// shows unless they set one there ([`crate::profile`]), replacing the one before.
    #[serde(rename = "account_avatar")]
    AccountAvatar,
}

/// What an app sends to start an upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct UploadRequest {
    /// What it's for.
    pub purpose: Purpose,
    /// Its name, as the person sees it.
    pub name: String,
    /// Its media type as the app knows it, from its name; zirc.dev checks the bytes.
    pub mime: String,
    /// Its size in bytes.
    pub size: u64,
    /// The channel it's shared in, for the organisation's Storage page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// The organisation's short name. An app's account says which it is; a browser
    /// says it here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
}

/// What zirc.dev answers an [`UploadRequest`] with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct UploadStarted {
    /// The file's id.
    pub id: String,
    /// Where to `PUT` its bytes.
    pub url: String,
    /// The headers to send with them, which the URL is signed for.
    pub headers: BTreeMap<String, String>,
    /// How many seconds the URL lasts.
    pub expires_in: u32,
}

/// The TypeScript declarations of these types, for [`crate::login::typescript`].
#[must_use]
pub(crate) fn typescript(config: &ts_rs::Config) -> Vec<String> {
    vec![
        Purpose::decl(config),
        format!("const MEDIA_SETTINGS_PATH = {SETTINGS_PATH:?};"),
        UploadRequest::decl(config),
        UploadStarted::decl(config),
        PreviewKind::decl(config),
        Preview::decl(config),
        crate::File::decl(config),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_read_as_the_web_app_writes_them() {
        let json =
            r##"{"purpose":"share","name":"a.png","mime":"image/png","size":3,"channel":"#api"}"##;
        let request: UploadRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.purpose, Purpose::Share);
        assert_eq!(request.org, None);
        assert_eq!(serde_json::to_string(&request).unwrap(), json);
        assert_eq!(done_path("abc"), "/api/media/abc/done");
    }
}
