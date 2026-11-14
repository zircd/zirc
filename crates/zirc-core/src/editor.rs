//! The editor a repository's files open in, from a message's code links
//! ([`zirc_chat::chat::CodeRef`]): each person's choice, kept by their app, or else the first
//! of [`Editor::ALL`] installed here. VS Code, Cursor and Zed open through their addresses,
//! which zirc-chat makes; Xcode through `xed`. And a checked-out file's text, for a preview
//! card, read here: repository code never leaves the machine for it.

use std::path::Path;
use std::process::Command;

pub use zirc_chat::chat::Editor;

/// The largest file a preview reads, so a huge generated file can't stall an app.
pub const PREVIEW_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// The editor files open in, as the person's setting `chosen` says
/// ([`zirc_chat::chat::editor_options`]' ids): that editor, none for the forge, or else the
/// first installed here.
#[must_use]
pub fn editor(chosen: Option<&str>) -> Option<Editor> {
    zirc_chat::chat::chosen_editor(chosen, &installed(&|path| path.exists()))
}

/// Where code can open here, as the "Open code in" picker lists it, `chosen` marked.
#[must_use]
pub fn options(chosen: Option<&str>) -> Vec<zirc_chat::chat::EditorOption> {
    zirc_chat::chat::editor_options(&installed(&|path| path.exists()), chosen)
}

/// What the setting keeps for the choice `id`: nothing for Automatic, which is the default.
#[must_use]
pub fn kept(id: &str) -> Option<String> {
    (id != zirc_chat::chat::EDITOR_AUTOMATIC).then(|| id.to_owned())
}

/// The editors installed here, in [`Editor::ALL`]'s order, by whether `exists` finds their
/// app (macOS) or their command on `PATH` (elsewhere).
#[must_use]
pub fn installed(exists: &dyn Fn(&Path) -> bool) -> Vec<Editor> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let path = std::env::var_os("PATH").unwrap_or_default();
    Editor::ALL
        .into_iter()
        .filter(|editor| {
            if cfg!(target_os = "macos") {
                let app = format!("{}.app", app_name(*editor));
                exists(&Path::new("/Applications").join(&app))
                    || home
                        .as_ref()
                        .is_some_and(|home| exists(&home.join("Applications").join(&app)))
            } else {
                commands(*editor).iter().any(|command| {
                    std::env::split_paths(&path).any(|dir| exists(&dir.join(command)))
                })
            }
        })
        .collect()
}

/// The app's name in `/Applications`.
fn app_name(editor: Editor) -> &'static str {
    match editor {
        Editor::VsCode => "Visual Studio Code",
        Editor::Cursor => "Cursor",
        Editor::Zed => "Zed",
        Editor::Xcode => "Xcode",
    }
}

/// The commands that start it, outside macOS.
fn commands(editor: Editor) -> &'static [&'static str] {
    match editor {
        Editor::VsCode => &["code"],
        Editor::Cursor => &["cursor"],
        Editor::Zed => &["zed", "zeditor"],
        Editor::Xcode => &[],
    }
}

/// The command that opens `file`, an absolute path, in `editor` at `line`, where its
/// address won't: Xcode's `xed --line`. `None` for the others, whose addresses an app
/// opens as any link.
#[must_use]
pub fn command(editor: Editor, file: &str, line: Option<u32>) -> Option<Command> {
    if editor != Editor::Xcode {
        return None;
    }
    let mut command = Command::new("xed");
    if let Some(line) = line {
        command.arg("--line").arg(line.to_string());
    }
    command.arg(file);
    Some(command)
}

/// Opens the file `code` names in `editor` when only a command can ([`command`]): whether
/// it did, so the app opens the link's address otherwise.
pub fn open(editor: Option<Editor>, code: &zirc_chat::chat::CodeRef) -> bool {
    let (Some(editor), Some(local)) = (editor, code.local.as_deref()) else {
        return false;
    };
    let Some(mut command) = command(editor, local, code.line) else {
        return false;
    };
    match command.spawn() {
        Ok(_) => true,
        Err(err) => {
            tracing::warn!("opening {local} in {}: {err}", editor.name());
            false
        }
    }
}

/// The text of the checked-out file `code` names, for its preview: `None` where it isn't
/// checked out here, is missing, is larger than [`PREVIEW_MAX_BYTES`] or isn't text.
#[must_use]
pub fn local_text(code: &zirc_chat::chat::CodeRef) -> Option<String> {
    let local = Path::new(code.local.as_deref()?);
    let size = std::fs::metadata(local).ok()?.len();
    if size > PREVIEW_MAX_BYTES {
        return None;
    }
    std::fs::read_to_string(local).ok()
}

/// One checkout for each repository's channel zircd watches, `(channel, root)`, for code
/// links ([`zirc_chat::chat::Chat::set_checkouts`]): its main one rather than a linked
/// worktree ([`checkouts_where`]).
#[must_use]
pub fn checkouts(repos: &[crate::ipc::Repo]) -> Vec<(String, String)> {
    checkouts_where(repos, &|root| root.join(".git").is_dir())
}

/// One checkout for each repository's channel in `repos`: its main one, whose `.git` is a
/// directory as `is_main` says, rather than a linked worktree, which comes and goes; or else
/// the first.
#[must_use]
pub fn checkouts_where(
    repos: &[crate::ipc::Repo],
    is_main: &dyn Fn(&std::path::Path) -> bool,
) -> Vec<(String, String)> {
    let mut chosen: Vec<(String, String, bool)> = Vec::new();
    for repo in repos {
        let main = is_main(&repo.path);
        let root = repo.path.to_string_lossy().into_owned();
        match chosen
            .iter_mut()
            .find(|(channel, ..)| *channel == repo.channel)
        {
            Some(slot) if main && !slot.2 => *slot = (repo.channel.clone(), root, main),
            Some(_) => {}
            None => chosen.push((repo.channel.clone(), root, main)),
        }
    }
    chosen
        .into_iter()
        .map(|(channel, root, _)| (channel, root))
        .collect()
}

/// How long a preview waits for the forge.
const FORGE_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The text of the file `code` names, for its preview: from its checkout here
/// ([`local_text`]), or else straight from its forge (`raw`), never through zirc.dev. `None`
/// when neither has it, as for a private repository's file not checked out here.
pub async fn text(code: &zirc_chat::chat::CodeRef) -> Option<String> {
    if let Some(text) = local_text(code) {
        return Some(text);
    }
    let raw = code.raw.as_deref()?;
    if !raw.starts_with("https://") {
        return None;
    }
    let client = reqwest::Client::builder()
        .timeout(FORGE_WAIT)
        .build()
        .ok()?;
    let answer = client.get(raw).send().await.ok()?.error_for_status().ok()?;
    if answer
        .content_length()
        .is_some_and(|length| length > PREVIEW_MAX_BYTES)
    {
        return None;
    }
    let bytes = answer.bytes().await.ok()?;
    if bytes.len() as u64 > PREVIEW_MAX_BYTES {
        return None;
    }
    String::from_utf8(bytes.to_vec()).ok()
}

/// The preview card of the file `code` names, highlighted, with where it opens: its lines
/// as [`text`] finds them, or why there are none.
pub async fn preview(
    code: &zirc_chat::chat::CodeRef,
    editor: Option<Editor>,
) -> zirc_chat::chat::CodePreview {
    use zirc_chat::chat::{CodePreview, code_actions, code_preview, highlighted_lines};
    let shown = text(code)
        .await
        .and_then(|text| code_preview(code, &text, Some(highlighted_lines)));
    match shown {
        Some(preview) => CodePreview {
            actions: code_actions(code, editor),
            ..preview
        },
        None => CodePreview::unavailable(code, editor),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_each_repositorys_main_checkout() {
        let repo = |path: &str, channel: &str| crate::ipc::Repo {
            path: path.into(),
            channel: channel.into(),
            sessions: Vec::new(),
            org: None,
            shared: Vec::new(),
        };
        let repos = [
            repo("/w/monorepo-worktrees/a", "#monorepo"),
            repo("/c/monorepo", "#monorepo"),
            repo("/w/monorepo-worktrees/b", "#monorepo"),
            repo("/c/api", "#api"),
        ];
        let main = |root: &std::path::Path| root.starts_with("/c");
        assert_eq!(
            checkouts_where(&repos, &main),
            [
                ("#monorepo".to_owned(), "/c/monorepo".to_owned()),
                ("#api".to_owned(), "/c/api".to_owned()),
            ]
        );
    }

    #[test]
    fn picks_the_chosen_editor_or_the_first_installed() {
        assert_eq!(editor(Some("zed")), Some(Editor::Zed));
        assert_eq!(editor(Some("forge")), None);
        assert_eq!(kept("automatic"), None);
        assert_eq!(kept("forge").as_deref(), Some("forge"));
        let found = installed(&|path| {
            let path = path.to_string_lossy();
            path.contains("Cursor") || path.ends_with("/cursor") || path.contains("Xcode")
        });
        assert_eq!(found.first(), Some(&Editor::Cursor));
    }

    #[test]
    fn opens_xcode_with_xed_and_the_rest_by_address() {
        let xed = command(Editor::Xcode, "/src/a.rs", Some(4)).unwrap();
        assert_eq!(xed.get_program(), "xed");
        let args: Vec<_> = xed.get_args().collect();
        assert_eq!(args, ["--line", "4", "/src/a.rs"]);
        assert!(command(Editor::VsCode, "/src/a.rs", Some(4)).is_none());
    }

    #[test]
    fn reads_a_checked_out_files_text_for_its_preview() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        let code = zirc_chat::chat::CodeRef {
            path: Some("a.rs".into()),
            line: Some(1),
            column: None,
            commit: None,
            local: Some(file.to_string_lossy().into_owned()),
            raw: None,
            page: None,
        };
        assert_eq!(local_text(&code).as_deref(), Some("fn main() {}\n"));
        let missing = zirc_chat::chat::CodeRef {
            local: Some(dir.path().join("b.rs").to_string_lossy().into_owned()),
            ..code
        };
        assert_eq!(local_text(&missing), None);
    }
}
