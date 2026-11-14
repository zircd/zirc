//! Video for zirc's files, as zirc.dev (web/src/lib/media-video.ts) or a self-hosted server
//! (`oss/crates/zirc-server`) asks: what a video is
//! (its length, size and codecs), its poster frame, and copies a browser or phone plays
//! (H.264 and AAC in MP4, at most 480, 720 or 1080 lines, starting to play before they're
//! all there). Nothing here has access to any bucket: zirc.dev hands over a short-lived URL
//! to read the original from and one to put what's made to, so an organisation's own bucket
//! needs nothing here either. Everything made leaves out the original's metadata, location
//! included.
//!
//! Making a copy takes a while, so it runs apart from the request that asked: the request
//! answers at once, and zirc.dev looks for the copy where it said to put it. A copy already
//! being made isn't started twice, and only a few run at once.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::Semaphore;
use tracing::{info, warn};

/// Where to put what's made: a signed URL and the headers it's signed for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Put {
    /// The URL to `PUT` it to.
    pub url: String,
    /// The headers the URL is signed for.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

/// What to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "kind")]
pub enum Make {
    /// A frame about a second in, as a JPEG.
    Poster,
    /// An MP4 at most `height` lines tall.
    Copy {
        /// The most lines it has: one of [`HEIGHTS`].
        height: u32,
    },
}

/// A job: what to make from `source`, and where to put it. `id` names it, so the same job
/// asked for twice runs once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    /// What names the job, the same for the same thing made from the same file.
    pub id: String,
    /// A URL to read the original from.
    pub source: String,
    /// What to make of it.
    pub make: Make,
    /// Where to put it.
    pub put: Put,
}

/// What a video is, as ffprobe reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probed {
    /// How long it plays, in milliseconds.
    pub duration_ms: u64,
    /// Its width, in pixels, as it shows.
    pub width: u32,
    /// Its height, in pixels, as it shows.
    pub height: u32,
    /// The video's codec, such as `h264`, and the container's names, such as `mov,mp4`.
    pub video: String,
    /// Its audio's codec, such as `aac`, if it has any.
    pub audio: Option<String>,
    /// The container's names, such as `mov,mp4`.
    pub container: String,
}

/// The heights copies are made at. web/src/lib/media-video.ts asks for these only.
pub const HEIGHTS: [u32; 3] = [480, 720, 1080];

/// The height of the copy to play for about `asked` lines: the nearest of [`HEIGHTS`] at
/// most that, or the smallest. web/src/lib/media-video.ts's `heightFor` is a copy.
#[must_use]
pub fn height_for(asked: u32) -> u32 {
    HEIGHTS
        .into_iter()
        .rev()
        .find(|&height| height <= asked)
        .unwrap_or(HEIGHTS[0])
}

/// Why something couldn't be made.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// ffmpeg or ffprobe failed, saying this.
    #[error("ffmpeg or ffprobe: {0}")]
    Tool(String),
    /// What ffprobe said wasn't what it says.
    #[error("reading what ffprobe said: {0}")]
    Json(#[from] serde_json::Error),
    /// What was made couldn't be sent.
    #[error("putting what was made: {0}")]
    Http(#[from] reqwest::Error),
    /// Storage refused what was made.
    #[error("putting what was made: storage answered {0}")]
    Status(reqwest::StatusCode),
    /// The scratch directory couldn't be used.
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// The containers a video may come in, as ffmpeg names their readers: MP4 and `QuickTime`
/// (one reader), and `WebM` (Matroska's), which are what zirc.dev plays (zirc-proto's
/// `uploads.rs`). What's uploaded says it's one of those, but its bytes say what it is to
/// ffmpeg, which reads by content: a playlist (ffconcat, a manifest) passed for a video and
/// had ffmpeg fetch whatever addresses it listed, from inside the cluster. Given to both
/// tools before the source, nothing else is opened.
pub const FORMATS: &str = "mov,mp4,m4a,3gp,3g2,mj2,matroska,webm";

/// How long ffprobe may take to say what a file is.
const PROBE_WITHIN: std::time::Duration = std::time::Duration::from_mins(1);

/// How long ffmpeg may take to make a poster or a copy: longer, and the job is given up,
/// so one file that never ends doesn't keep the worker's one place for good.
const MAKE_WITHIN: std::time::Duration = std::time::Duration::from_mins(20);

/// ffprobe's arguments for `source`.
#[must_use]
pub fn probe_args(source: &str) -> Vec<String> {
    [
        "-v",
        "error",
        "-format_whitelist",
        FORMATS,
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
        source,
    ]
    .map(str::to_owned)
    .to_vec()
}

/// What ffprobe's JSON says about a video, or `None` when it has no video stream.
///
/// # Errors
///
/// When it isn't ffprobe's JSON.
pub fn probed(json: &str) -> Result<Option<Probed>, serde_json::Error> {
    #[derive(Deserialize)]
    struct Output {
        #[serde(default)]
        streams: Vec<Stream>,
        format: Format,
    }
    #[derive(Deserialize)]
    struct Stream {
        codec_type: String,
        codec_name: Option<String>,
        width: Option<u32>,
        height: Option<u32>,
        #[serde(default)]
        side_data_list: Vec<serde_json::Value>,
        #[serde(default)]
        tags: BTreeMap<String, String>,
    }
    #[derive(Deserialize)]
    struct Format {
        format_name: String,
        duration: Option<String>,
    }
    let output: Output = serde_json::from_str(json)?;
    let Some(video) = output.streams.iter().find(|s| s.codec_type == "video") else {
        return Ok(None);
    };
    let audio = output.streams.iter().find(|s| s.codec_type == "audio");
    // A phone's portrait video is landscape turned a quarter: its size as it's shown.
    let rotation = video
        .side_data_list
        .iter()
        .find_map(|d| d.get("rotation").and_then(serde_json::Value::as_i64))
        .or_else(|| video.tags.get("rotate").and_then(|r| r.parse().ok()))
        .unwrap_or(0);
    let (width, height) = (video.width.unwrap_or(0), video.height.unwrap_or(0));
    let (width, height) = if rotation.rem_euclid(180) == 90 {
        (height, width)
    } else {
        (width, height)
    };
    let seconds: f64 = output
        .format
        .duration
        .as_deref()
        .and_then(|d| d.parse().ok())
        .unwrap_or(0.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "whole milliseconds of a length well inside u64"
    )]
    let duration_ms = (seconds * 1000.0).round().max(0.0) as u64;
    Ok(Some(Probed {
        duration_ms,
        width,
        height,
        video: video.codec_name.clone().unwrap_or_default(),
        audio: audio.and_then(|a| a.codec_name.clone()),
        container: output.format.format_name,
    }))
}

/// ffmpeg's arguments to make `make` from `source` into `out`, without the original's
/// metadata, location included.
#[must_use]
pub fn ffmpeg_args(source: &str, make: Make, out: &str) -> Vec<String> {
    // The formats before the source they're for.
    let mut args: Vec<String> = [
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-format_whitelist",
        FORMATS,
    ]
    .map(str::to_owned)
    .to_vec();
    match make {
        Make::Poster => {
            // A second in, where a video has usually got going; the first frame of a shorter one.
            args.extend(
                [
                    "-ss",
                    "1",
                    "-i",
                    source,
                    "-frames:v",
                    "1",
                    "-map_metadata",
                    "-1",
                    "-q:v",
                    "3",
                    out,
                ]
                .map(str::to_owned),
            );
        }
        Make::Copy { height } => {
            let scale = format!("scale=-2:'min({height},ih)'");
            args.extend(
                [
                    "-i",
                    source,
                    "-map",
                    "0:v:0",
                    "-map",
                    "0:a:0?",
                    "-map_metadata",
                    "-1",
                    "-map_chapters",
                    "-1",
                    "-vf",
                    &scale,
                    "-c:v",
                    "libx264",
                    "-preset",
                    "veryfast",
                    "-crf",
                    "23",
                    "-pix_fmt",
                    "yuv420p",
                    "-c:a",
                    "aac",
                    "-b:a",
                    "128k",
                    "-movflags",
                    "+faststart",
                    out,
                ]
                .map(str::to_owned),
            );
        }
    }
    args
}

/// Runs a tool, answering what it printed, or ending it if it takes longer than `within`.
async fn run(tool: &str, args: &[String], within: std::time::Duration) -> Result<String, Error> {
    let running = Command::new(tool).args(args).kill_on_drop(true).output();
    let Ok(output) = tokio::time::timeout(within, running).await else {
        return Err(Error::Tool(format!(
            "{tool} took longer than {}s and was stopped",
            within.as_secs()
        )));
    };
    let output = output?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(Error::Tool(
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .take(5)
                .collect::<Vec<_>>()
                .join(" "),
        ))
    }
}

/// What the video at `source` is: `None` for something that isn't one.
///
/// # Errors
///
/// When ffprobe can't read it.
pub async fn probe(source: &str) -> Result<Option<Probed>, Error> {
    Ok(probed(
        &run("ffprobe", &probe_args(source), PROBE_WITHIN).await?,
    )?)
}

/// The jobs running and waiting, and how many may run at once.
#[derive(Clone)]
pub struct Worker {
    running: Arc<Mutex<HashSet<String>>>,
    slots: Arc<Semaphore>,
    scratch: Arc<std::path::PathBuf>,
}

impl Worker {
    /// A worker making at most `at_once` things at a time, in `scratch` meanwhile.
    #[must_use]
    pub fn new(at_once: usize, scratch: &Path) -> Self {
        Self {
            running: Arc::default(),
            slots: Arc::new(Semaphore::new(at_once.max(1))),
            scratch: Arc::new(scratch.to_owned()),
        }
    }

    /// Starts `job` unless it's running or waiting already: true when it started.
    pub fn start(&self, job: Job) -> bool {
        if !self
            .running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(job.id.clone())
        {
            return false;
        }
        let worker = self.clone();
        tokio::spawn(async move {
            let _slot = worker.slots.acquire().await;
            let id = job.id.clone();
            match worker.make(&job).await {
                Ok(bytes) => info!(job = id, bytes, "made"),
                Err(error) => warn!(job = id, %error, "couldn't make it"),
            }
            worker
                .running
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&id);
        });
        true
    }

    async fn make(&self, job: &Job) -> Result<u64, Error> {
        let name: String = job
            .id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let out = self.scratch.join(match job.make {
            Make::Poster => format!("{name}.jpg"),
            Make::Copy { .. } => format!("{name}.mp4"),
        });
        let out_text = out.to_string_lossy().into_owned();
        let made = async {
            let args = ffmpeg_args(&job.source, job.make, &out_text);
            run("ffmpeg", &args, MAKE_WITHIN).await?;
            put(&out, &job.put).await
        }
        .await;
        let _ = tokio::fs::remove_file(&out).await;
        made
    }
}

/// PUTs the file at `path` to `put`, streaming it.
async fn put(path: &Path, put: &Put) -> Result<u64, Error> {
    let file = tokio::fs::File::open(path).await?;
    let length = file.metadata().await?.len();
    let mut request = reqwest::Client::new()
        .put(&put.url)
        .header(reqwest::header::CONTENT_LENGTH, length)
        .body(reqwest::Body::wrap_stream(
            tokio_util::io::ReaderStream::new(file),
        ));
    for (name, value) in &put.headers {
        request = request.header(name, value);
    }
    let response = request.send().await?;
    if !response.status().is_success() {
        return Err(Error::Status(response.status()));
    }
    Ok(length)
}

pub mod server;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_are_the_nearest_height_down_or_the_smallest() {
        assert_eq!(height_for(720), 720);
        assert_eq!(height_for(1000), 720);
        assert_eq!(height_for(4000), 1080);
        assert_eq!(height_for(100), 480);
    }

    #[test]
    fn reads_what_a_video_is_turned_as_it_shows() {
        let json = r#"{"streams":[
            {"codec_type":"video","codec_name":"hevc","width":1920,"height":1080,
             "side_data_list":[{"side_data_type":"Display Matrix","rotation":-90}]},
            {"codec_type":"audio","codec_name":"aac"}],
          "format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"12.345678",
                    "tags":{"location":"+51.5000-000.1200/"}}}"#;
        assert_eq!(
            probed(json).unwrap(),
            Some(Probed {
                duration_ms: 12346,
                width: 1080,
                height: 1920,
                video: "hevc".into(),
                audio: Some("aac".into()),
                container: "mov,mp4,m4a,3gp,3g2,mj2".into(),
            })
        );
    }

    #[test]
    fn knows_audio_alone_isnt_a_video() {
        let json = r#"{"streams":[{"codec_type":"audio","codec_name":"mp3"}],"format":{"format_name":"mp3"}}"#;
        assert_eq!(probed(json).unwrap(), None);
    }

    #[test]
    fn copies_leave_out_metadata_and_fit_their_height() {
        let args = ffmpeg_args(
            "https://storage/x",
            Make::Copy { height: 720 },
            "/tmp/a.mp4",
        );
        let joined = args.join(" ");
        assert!(joined.contains("-map_metadata -1"), "{joined}");
        assert!(joined.contains("scale=-2:'min(720,ih)'"), "{joined}");
        assert!(joined.contains("-movflags +faststart"), "{joined}");
        assert!(joined.ends_with("/tmp/a.mp4"));
        // Only the containers a video comes in are read, said before the source.
        let listed = format!("-format_whitelist {FORMATS} ");
        let source = joined.find("-i https://storage/x").unwrap();
        let before = joined.find(&listed).is_some_and(|at| at < source);
        assert!(before, "{joined}");
        assert!(probe_args("https://storage/x").join(" ").contains(&listed));
        let poster = ffmpeg_args("https://storage/x", Make::Poster, "/tmp/a.jpg").join(" ");
        assert!(poster.contains(&listed), "{poster}");
        assert!(
            poster.contains("-frames:v 1") && poster.contains("-map_metadata -1"),
            "{poster}"
        );
    }

    #[test]
    fn jobs_read_as_zirc_dev_writes_them() {
        let json = r#"{"id":"abc/720","source":"https://s/o","make":{"kind":"copy","height":720},
                       "put":{"url":"https://s/p","headers":{"content-type":"video/mp4"}}}"#;
        let job: Job = serde_json::from_str(json).unwrap();
        assert_eq!(job.make, Make::Copy { height: 720 });
        assert_eq!(job.put.headers["content-type"], "video/mp4");
    }
}
