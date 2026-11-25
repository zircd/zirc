//! A session's title as its agent tool shows it, read from the tool's own records each time
//! `zirc watch` starts, so the title zirc shows keeps in step when the session is renamed
//! (the user's choice, 25 Nov 2026). Only the title is read and sent: nothing else of the
//! transcript leaves it.
//!
//! Claude Code keeps it in the session's transcript,
//! `<config>/projects/<project>/<session id>.jsonl`, as `{"type":"custom-title",
//! "customTitle": …, "sessionId": …}` lines, which it writes again near the end as the
//! session goes on (in the eight largest transcripts on one Mac, up to 168 MB, the last was
//! always within the last 30 KB), the last one being what its sidebar shows. Its commands
//! carry the session's id as `CLAUDE_CODE_SESSION_ID`. No other tool keeps a title zirc can
//! read yet.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// How much of a transcript's end is read for its title.
const TAIL: u64 = 1 << 20;

/// How a title line starts, as Claude Code writes it: lines that don't, by far the most, are
/// never parsed.
const TITLE_LINE: &str = r#"{"type":"custom-title","#;

/// The title of the session zirc runs in, as `tool` shows it: `None` when the tool keeps
/// none zirc reads, or it can't be read.
pub(crate) fn here(tool: Option<&str>) -> Option<String> {
    if tool != Some(zirc_proto::CLAUDE_CODE) {
        return None;
    }
    let id = std::env::var("CLAUDE_CODE_SESSION_ID").ok()?;
    let config = crate::skill::claude_config_dir().ok()?;
    let transcript = transcript(&config, &id)?;
    custom_title(&tail(&transcript)?, &id)
}

/// The transcript of Claude Code's session `id` under `config`, in whichever project's
/// directory it is: by the id alone, as the project's is named from where the session
/// started, which needn't be where zirc runs.
fn transcript(config: &Path, id: &str) -> Option<PathBuf> {
    // An id is a UUID: anything else could name a path elsewhere.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return None;
    }
    let file = format!("{id}.jsonl");
    std::fs::read_dir(config.join("projects"))
        .ok()?
        .flatten()
        .map(|project| project.path().join(&file))
        .find(|path| path.is_file())
}

/// The last [`TAIL`] bytes of `path`, from its first whole line.
fn tail(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let from = len.saturating_sub(TAIL);
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Some(match (from, text.split_once('\n')) {
        (0, _) | (_, None) => text,
        (_, Some((_, rest))) => rest.to_owned(),
    })
}

/// The last title `transcript` gives session `id`, as Claude Code's sidebar shows it.
fn custom_title(transcript: &str, id: &str) -> Option<String> {
    transcript
        .lines()
        .rev()
        .filter(|line| line.starts_with(TITLE_LINE))
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|line| line["sessionId"] == id)
        .and_then(|line| zirc_proto::session::title(line["customTitle"].as_str()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "d44789fc-a1be-480e-b9ab-13c419649c93";

    #[test]
    fn reads_the_last_title_claude_code_gave_the_session() {
        let transcript = format!(
            "{{\"type\":\"custom-title\",\"customTitle\":\"Session names\",\"sessionId\":\"{ID}\"}}\n\
             {{\"type\":\"user\",\"message\":{{\"content\":\"{{\\\"type\\\":\\\"custom-title\\\",\\\"customTitle\\\":\\\"quoted\\\"}}\"}}}}\n\
             {{\"type\":\"custom-title\",\"customTitle\":\"Session names:  tool and title\",\"sessionId\":\"{ID}\"}}\n\
             {{\"type\":\"custom-title\",\"customTitle\":\"Another session\",\"sessionId\":\"0a142e29\"}}\n\
             {{\"type\":\"assistant\"}}\n"
        );
        assert_eq!(
            custom_title(&transcript, ID).as_deref(),
            Some("Session names: tool and title")
        );
        assert_eq!(custom_title("{\"type\":\"user\"}\n", ID), None);
        assert_eq!(
            custom_title(&transcript, "0a142e29").as_deref(),
            Some("Another session")
        );
    }

    #[test]
    fn finds_the_transcript_in_any_project_by_the_session_id_alone() {
        let config = tempfile::tempdir().unwrap();
        let project = config.path().join("projects/-Users-me-src-app");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(config.path().join("projects/-Users-me-other")).unwrap();
        let path = project.join(format!("{ID}.jsonl"));
        // A long transcript: its title is near its end, past the part read.
        let filler = "{\"type\":\"assistant\",\"text\":\"…\"}\n".repeat(40_000);
        let old = format!(
            "{{\"type\":\"custom-title\",\"customTitle\":\"Old\",\"sessionId\":\"{ID}\"}}\n"
        );
        let new = format!(
            "{{\"type\":\"custom-title\",\"customTitle\":\"Renamed\",\"sessionId\":\"{ID}\"}}\n"
        );
        std::fs::write(&path, format!("{old}{filler}{new}{}", &filler[..2000])).unwrap();
        assert_eq!(transcript(config.path(), ID), Some(path.clone()));
        assert_eq!(transcript(config.path(), "../../etc/passwd"), None);
        assert_eq!(
            custom_title(&tail(&path).unwrap(), ID).as_deref(),
            Some("Renamed")
        );
    }
}
