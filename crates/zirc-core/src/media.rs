//! Uploading files to zirc.dev and removing them, for every zirc app alike. See
//! [`zirc_proto::media`] for the steps.

use std::time::Duration;

use reqwest::StatusCode;
use zirc_proto::File;
use zirc_proto::login::Account;
use zirc_proto::media::{self, Preview, UploadRequest, UploadStarted};

use crate::login::{Client, Error, passing};

/// How many times the bytes are sent before giving up, when storage is away for a moment.
const ATTEMPTS: u32 = 3;

/// How long an upload may take: larger than a request's usual time, for a large file on a
/// slow line.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(600);

impl Client {
    /// Uploads `bytes` as `request` says, as `account`, and answers the file to share: with
    /// its type from its bytes and, for an image, its size once re-encoded. Storage being
    /// away for a moment only means sending again, a few times.
    ///
    /// # Errors
    ///
    /// When the web app refuses it, saying why (switched off, too large, over the quota,
    /// not what it said it was), or can't be reached.
    pub async fn upload(
        &self,
        account: &Account,
        request: &UploadRequest,
        bytes: Vec<u8>,
    ) -> Result<File, Error> {
        let url = format!("{}{}", self.web, media::UPLOAD_PATH);
        let response = self
            .http
            .post(url)
            .basic_auth(&account.name, Some(&account.password))
            .json(request)
            .send()
            .await?;
        let started: UploadStarted = self.checked(response).await?.json().await?;
        let mut attempt = 1;
        loop {
            match self.put(&started, bytes.clone()).await {
                Ok(()) => break,
                Err(err) if attempt < ATTEMPTS && passing(&err) => {
                    tokio::time::sleep(Duration::from_secs(u64::from(attempt))).await;
                    attempt += 1;
                }
                Err(err) => return Err(err),
            }
        }
        let url = format!("{}{}", self.web, media::done_path(&started.id));
        let response = self
            .http
            .post(url)
            .basic_auth(&account.name, Some(&account.password))
            .send()
            .await?;
        Ok(self.checked(response).await?.json().await?)
    }

    /// Removes the file with id `id`, and everything made from it, as `account`: its
    /// uploader or one of the organisation's admins. False when it isn't there, or isn't
    /// theirs to remove.
    ///
    /// # Errors
    ///
    /// When the web app can't be reached or answers anything else.
    pub async fn remove(&self, account: &Account, id: &str) -> Result<bool, Error> {
        let url = format!("{}{}", self.web, media::file_path(id));
        let response = self
            .http
            .delete(url)
            .basic_auth(&account.name, Some(&account.password))
            .send()
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(false);
        }
        self.checked(response).await?;
        Ok(true)
    }

    /// A file's bytes, from its [`File::url`] or an image's at a width
    /// ([`crate::chat::file::image_url`]), as `account`: zirc.dev redirects to where they
    /// are, and the account goes no further than zirc.dev. What zirc.dev answers decides
    /// [`Download::Removed`], and nothing else does: a video's poster still being made is
    /// [`Download::Making`], and not reaching zirc.dev an error.
    ///
    /// # Errors
    ///
    /// When the link isn't on the web app ([`Error::Foreign`]): a file's link is whatever its
    /// sender wrote, and the account must never reach another host. When the web app or
    /// storage can't be reached or answers anything else.
    pub async fn download(&self, account: &Account, url: &str) -> Result<Download, Error> {
        if !url.starts_with(&format!("{}/", self.web)) {
            return Err(Error::Foreign {
                web: self.web.clone(),
                url: url.to_owned(),
            });
        }
        let response = self
            .http
            .get(url)
            .basic_auth(&account.name, Some(&account.password))
            .timeout(UPLOAD_TIMEOUT)
            .send()
            .await?;
        match response.status() {
            StatusCode::NOT_FOUND | StatusCode::GONE => return Ok(Download::Removed),
            StatusCode::ACCEPTED => return Ok(Download::Making),
            _ => {}
        }
        Ok(Download::Got(
            self.checked(response).await?.bytes().await?.to_vec(),
        ))
    }

    /// The preview of the text or Markdown file with id `id`, as `account`: `None` when it has
    /// none, or was removed.
    ///
    /// # Errors
    ///
    /// When the web app can't be reached or answers anything else.
    pub async fn preview(&self, account: &Account, id: &str) -> Result<Option<Preview>, Error> {
        let url = format!("{}{}", self.web, media::preview_path(id));
        let response = self
            .http
            .get(url)
            .basic_auth(&account.name, Some(&account.password))
            .send()
            .await?;
        if matches!(response.status(), StatusCode::NOT_FOUND | StatusCode::GONE) {
            return Ok(None);
        }
        Ok(Some(self.checked(response).await?.json().await?))
    }

    /// `nick`'s avatar, `pixels` wide at least, as `account`, in its organisation: a person's,
    /// or a machine's owner's. `None` when they have none, for their initials.
    ///
    /// # Errors
    ///
    /// As [`Client::download`].
    pub async fn avatar(
        &self,
        account: &Account,
        nick: &str,
        pixels: u32,
    ) -> Result<Option<Vec<u8>>, Error> {
        let url = format!(
            "{}{}?w={}",
            self.web,
            media::avatar_path(nick),
            crate::chat::file::width_for(pixels)
        );
        Ok(self.download(account, &url).await?.bytes())
    }

    /// Sends the bytes to the signed URL storage gave.
    async fn put(&self, started: &UploadStarted, bytes: Vec<u8>) -> Result<(), Error> {
        let mut put = self.http.put(&started.url).timeout(UPLOAD_TIMEOUT);
        for (name, value) in &started.headers {
            put = put.header(name, value);
        }
        let response = put.body(bytes).send().await?;
        self.checked(response).await?;
        Ok(())
    }
}

/// What zirc.dev answers for a file's bytes: the one place a zirc app learns a file was
/// removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Download {
    /// Its bytes.
    Got(Vec<u8>),
    /// Being made, as a video's poster is the first time it's asked for (zirc.dev's 202):
    /// ask again in a few seconds.
    Making,
    /// Removed, or never there: zirc.dev's 410 or 404. Only this says a file is gone.
    Removed,
}

impl Download {
    /// The bytes, if it has them.
    #[must_use]
    pub fn bytes(self) -> Option<Vec<u8>> {
        match self {
            Self::Got(bytes) => Some(bytes),
            Self::Making | Self::Removed => None,
        }
    }
}

/// The media type a file's name suggests, for an [`UploadRequest`]: zirc.dev checks its
/// bytes, so this only has to be a good guess.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guesses_types_from_names() {
        assert_eq!(guess_mime("IMG_2041.JPEG"), "image/jpeg");
        assert_eq!(guess_mime("notes.md"), "text/markdown");
        assert_eq!(guess_mime("Makefile"), "application/octet-stream");
    }

    #[tokio::test]
    async fn downloads_send_the_account_only_to_the_web_app() {
        let (web, served) = zirc_test_ergo::web::serve(vec![(200, "bytes".into())]);
        let client = Client::new(&web).unwrap();
        let account = Account {
            name: "mattias".into(),
            password: "secret".into(),
            device_key: false,
        };
        for foreign in [
            "https://evil.example/m/acme/1".to_owned(),
            format!("{web}.evil.example/m/acme/1"),
            format!("{web}@evil.example/m/acme/1"),
        ] {
            let err = client.download(&account, &foreign).await.unwrap_err();
            assert!(matches!(err, Error::Foreign { .. }), "{foreign}: {err}");
        }
        let bytes = client.download(&account, &format!("{web}/m/acme/1")).await;
        assert_eq!(bytes.unwrap(), Download::Got(b"bytes".to_vec()));
        assert_eq!(served.join().unwrap().len(), 1);
    }

    /// Only zirc.dev's 404 or 410 says a file was removed: a poster being made is a 202,
    /// whose body isn't the picture, and anything else is an error to try again after.
    #[tokio::test]
    async fn only_zirc_dev_says_a_file_was_removed() {
        let (web, served) = zirc_test_ergo::web::serve(vec![
            (202, r#"{"processing":true}"#.into()),
            (404, r#"{"error":"No such file."}"#.into()),
            (410, r#"{"error":"Removed by dana on 2026-11-03."}"#.into()),
            (500, "oops".into()),
        ]);
        let client = Client::new(&web).unwrap();
        let account = Account {
            name: "mattias".into(),
            password: "secret".into(),
            device_key: false,
        };
        let url = format!("{web}/m/acme/1?w=640");
        assert_eq!(
            client.download(&account, &url).await.unwrap(),
            Download::Making
        );
        assert_eq!(
            client.download(&account, &url).await.unwrap(),
            Download::Removed
        );
        assert_eq!(
            client.download(&account, &url).await.unwrap(),
            Download::Removed
        );
        assert!(client.download(&account, &url).await.is_err());
        assert_eq!(served.join().unwrap().len(), 4);
    }
}
