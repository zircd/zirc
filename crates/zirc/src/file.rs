//! `zirc file`: saves a file someone shared, so an agent can read it. A message shows a
//! shared file as its link, `https://zirc.dev/m/<organisation>/<id>`, which only a member's
//! sign-in opens: a person's browser has one, and an agent has none. This machine's own
//! account is one zirc.dev takes for its organisation's files, as it does an app's, so the
//! command fetches the file as that and writes it where the agent can open it. Until 23 Nov
//! 2026 an agent could see that a picture was shared and not look at it (the user's
//! question, on attaching pictures to messages).

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use clap::Args;
use zirc_core::media::Download;

use crate::search::{one_org, team};

#[derive(Debug, Args)]
pub struct FileArgs {
    /// The file's link, as the message shows it: it ends /m/ORGANISATION/ID.
    link: String,
    /// Where to save it: a file, or a folder to save it in under its own name. Without it,
    /// a folder of zirc's in the system's temporary files.
    #[arg(long, value_name = "PATH")]
    out: Option<PathBuf>,
    /// For a picture, how many pixels wide to fetch it at most: zirc.dev resizes it, so a
    /// phone's photo of several megabytes comes small enough to read. Without it, a
    /// picture at the size zirc.dev serves one by itself, and any other file as it was
    /// shared.
    #[arg(long, value_name = "PIXELS")]
    width: Option<u32>,
    /// The organisation it was shared in, when the link doesn't say.
    #[arg(long, value_name = "NAME")]
    org: Option<String>,
}

/// The organisation and the file's id a shared file's `link` names: the two parts after
/// `/m/`, with nothing after them but a query.
fn named(link: &str) -> Option<(&str, &str)> {
    let path = link.split(['?', '#']).next()?;
    let (_, rest) = path.split_once("/m/")?;
    let (org, id) = rest.split_once('/')?;
    let plain = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_@.".contains(c))
    };
    (plain(org) && plain(id)).then_some((org, id))
}

/// What a file of these `bytes` is called by its type, when they're a picture zirc knows:
/// its media type, its size in pixels, and the extension its name takes.
fn pictured(bytes: &[u8]) -> Option<(String, &'static str)> {
    let probed = zirc_proto::images::probe(bytes)?;
    let extension = match probed.mime.as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "img",
    };
    Some((
        format!("{}, {}×{}", probed.mime, probed.width, probed.height),
        extension,
    ))
}

/// Runs `zirc file`.
pub async fn file(home: &Path, socket: &Path, args: FileArgs, out: &mut impl Write) -> Result<()> {
    let (slug, id) = named(&args.link).context(
        "that isn't a shared file's link: one reads https://zirc.dev/m/<organisation>/<id>",
    )?;
    let org = one_org(home, socket, Some(args.org.as_deref().unwrap_or(slug))).await?;
    let team = team(&org).with_context(|| {
        format!(
            "{} isn't signed in through zirc.dev here, which keeps its files",
            org.name()
        )
    })?;
    let link = args.link.split(['?', '#']).next().unwrap_or(&args.link);
    let url = match args.width {
        Some(pixels) => format!("{link}?w={}", zirc_proto::uploads::width_for(pixels)),
        None => link.to_owned(),
    };
    let bytes = match team.client.download(&team.account, &url).await? {
        Download::Got(bytes) => bytes,
        Download::Making => bail!("it's still being made: ask again in a few seconds"),
        Download::Removed => bail!("that file was removed, or was never there"),
    };
    let pictured = pictured(&bytes);
    let name = format!(
        "{id}.{}",
        pictured.as_ref().map_or("bin", |(_, extension)| extension)
    );
    let path = match args.out {
        Some(given) if given.is_dir() => given.join(name),
        Some(given) => given,
        None => {
            let folder = std::env::temp_dir().join("zirc-files");
            std::fs::create_dir_all(&folder)
                .with_context(|| format!("making {}", folder.display()))?;
            folder.join(name)
        }
    };
    std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
    let what = pictured.map_or_else(|| "a file".to_owned(), |(what, _)| what);
    let size = zirc_proto::uploads::size_text(bytes.len() as u64);
    writeln!(out, "saved {} ({what}, {size})", path.display())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shared_files_link_names_its_organisation_and_id() {
        assert_eq!(
            named("https://zirc.dev/m/acme/7Hq2xV9k"),
            Some(("acme", "7Hq2xV9k"))
        );
        assert_eq!(
            named("https://zirc.dev/m/acme/7Hq2xV9k?w=640"),
            Some(("acme", "7Hq2xV9k"))
        );
        assert_eq!(
            named("https://files.acme.example/m/acme/a-b_c"),
            Some(("acme", "a-b_c"))
        );
        // Not a file's link: a page, a link with more after the id, or one with neither.
        assert_eq!(named("https://zirc.dev/install"), None);
        assert_eq!(named("https://zirc.dev/m/acme/7Hq2xV9k/extra"), None);
        assert_eq!(named("https://zirc.dev/m/acme/"), None);
    }

    #[test]
    fn a_picture_is_named_by_its_type_and_anything_else_is_a_file() {
        let png = [
            b"\x89PNG\r\n\x1a\n".as_slice(),
            &13u32.to_be_bytes(),
            b"IHDR",
            &640u32.to_be_bytes(),
            &480u32.to_be_bytes(),
            &[8, 6, 0, 0, 0],
        ]
        .concat();
        assert_eq!(
            pictured(&png),
            Some(("image/png, 640×480".to_owned(), "png"))
        );
        assert_eq!(pictured(b"hello"), None);
    }
}
