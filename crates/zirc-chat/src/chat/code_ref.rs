//! Code named in a message: a repository's file, `crates/zirc-chat/src/chat.rs:42`, or
//! a commit, `3f2a1c0`, written bare or in backticks, becomes a link: into the person's
//! editor where the repository is checked out on their machine, or else to its page on the
//! repository's forge. Stack traces and diffs pasted without a fence become code blocks, so
//! they're cut as long code is ([`super::CODE_LINES`]) and a diff is coloured. Every app
//! draws what this finds; none looks for code itself.
//!
//! It's strict: a path needs a `/` or a `:line`, and a file name with a lettered extension;
//! a commit is 7 to 40 lowercase hex digits with a digit and a letter among them, in a
//! channel whose repository's forge zirc knows. Version numbers, domains, prose such as
//! `and/or` or `e.g.`, and hex inside a link or a longer word stay as they are.

use std::borrow::Cow;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The editor a file opens in, where the repository is checked out: each person's own
/// choice, which zirc-core keeps, the first installed of these by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(rename_all = "kebab-case")]
pub enum Editor {
    /// Visual Studio Code, `vscode://file/…`.
    VsCode,
    /// Cursor, `cursor://file/…`.
    Cursor,
    /// Zed, `zed://file/…`.
    Zed,
    /// Xcode, which has no address of its own: `xed --line` opens it, and the link is the
    /// file's `file://` address.
    Xcode,
}

impl Editor {
    /// Every editor, in the order zirc picks the first installed of.
    pub const ALL: [Self; 4] = [Self::VsCode, Self::Cursor, Self::Zed, Self::Xcode];

    /// Its name, as settings show it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::VsCode => "VS Code",
            Self::Cursor => "Cursor",
            Self::Zed => "Zed",
            Self::Xcode => "Xcode",
        }
    }

    /// Its id in settings: `vs-code`, `cursor`, `zed`, `xcode`.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::VsCode => "vs-code",
            Self::Cursor => "cursor",
            Self::Zed => "zed",
            Self::Xcode => "xcode",
        }
    }

    /// The editor whose [`Editor::id`] is `id`.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|editor| editor.id() == id)
    }

    /// The address that opens `file`, an absolute path, at `line` and `column` in it.
    #[must_use]
    pub fn link(self, file: &str, line: Option<u32>, column: Option<u32>) -> String {
        let file = escape_path(file);
        let at = match (line, column) {
            (Some(line), Some(column)) => format!(":{line}:{column}"),
            (Some(line), None) => format!(":{line}"),
            _ => String::new(),
        };
        match self {
            Self::VsCode => format!("vscode://file{file}{at}"),
            Self::Cursor => format!("cursor://file{file}{at}"),
            Self::Zed => format!("zed://file{file}{at}"),
            Self::Xcode => format!("file://{file}"),
        }
    }
}

/// The setting that says where code links open, as every app's Settings words it.
pub const OPEN_CODE_IN: &str = "Open code in";

/// [`OPEN_CODE_IN`]'s choice that picks the first editor installed: also what an unset
/// setting means.
pub const EDITOR_AUTOMATIC: &str = "automatic";

/// [`OPEN_CODE_IN`]'s choice that opens files on the forge even where they're checked out.
pub const EDITOR_FORGE: &str = "forge";

/// One of [`OPEN_CODE_IN`]'s choices, as a picker lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct EditorOption {
    /// What the setting keeps: [`EDITOR_AUTOMATIC`], an [`Editor::id`] or [`EDITOR_FORGE`].
    pub id: String,
    /// What the picker says: `Automatic (VS Code)`, `Zed`, `The forge`.
    pub label: String,
    /// Whether it's the one chosen.
    pub chosen: bool,
}

/// [`OPEN_CODE_IN`]'s choices: Automatic, naming the editor it picks, then each editor
/// `installed`, then the forge. One chosen but not installed is still listed, so it shows.
#[must_use]
pub fn editor_options(installed: &[Editor], chosen: Option<&str>) -> Vec<EditorOption> {
    let chosen = chosen.unwrap_or(EDITOR_AUTOMATIC);
    let automatic = match installed.first() {
        Some(editor) => format!("Automatic ({})", editor.name()),
        None => "Automatic (the forge)".to_owned(),
    };
    let mut editors: Vec<Editor> = installed.to_vec();
    if let Some(editor) = Editor::from_id(chosen).filter(|editor| !editors.contains(editor)) {
        editors.push(editor);
    }
    let option = |id: &str, label: String| EditorOption {
        id: id.to_owned(),
        label,
        chosen: chosen == id,
    };
    std::iter::once(option(EDITOR_AUTOMATIC, automatic))
        .chain(
            editors
                .into_iter()
                .map(|editor| option(editor.id(), editor.name().to_owned())),
        )
        .chain(std::iter::once(option(
            EDITOR_FORGE,
            "The forge".to_owned(),
        )))
        .collect()
}

/// The editor files open in, as the setting `chosen` says ([`editor_options`]'s ids): that
/// editor, none for the forge, or else the first `installed`.
#[must_use]
pub fn chosen_editor(chosen: Option<&str>, installed: &[Editor]) -> Option<Editor> {
    match chosen {
        Some(EDITOR_FORGE) => None,
        Some(id) if Editor::from_id(id).is_some() => Editor::from_id(id),
        _ => installed.first().copied(),
    }
}

/// Code a span of a message names, as every app's records carry it beside the span's link
/// ([`super::RichSpan::code`]): what a preview card shows and where it reads it from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CodeRef {
    /// The file's path in the repository, `crates/zirc-chat/src/chat.rs`; `None` for a
    /// commit.
    pub path: Option<String>,
    /// The line it names, from 1.
    pub line: Option<u32>,
    /// The column it names, from 1.
    pub column: Option<u32>,
    /// The commit it names, as written; `None` for a file.
    pub commit: Option<String>,
    /// The file on this machine, in the repository's checkout, for a preview read from
    /// disk: `None` where it isn't checked out, or for a commit.
    pub local: Option<String>,
    /// The file's text on the forge, for a preview where there's no checkout:
    /// `https://raw.githubusercontent.com/…/HEAD/<path>`. `None` when the forge isn't known.
    pub raw: Option<String>,
    /// Its page on the forge, which a preview card offers beside the editor.
    pub page: Option<String>,
}

/// What code links where, as the app says: each repository's checkout on this machine and
/// the editor files open in. Each repository's forge zirc learns from what's said.
#[derive(Debug, Clone, Default)]
pub(super) struct Known {
    /// Repository name → its web address on its forge.
    forges: HashMap<String, String>,
    /// Repository name → its checkout on this machine.
    checkouts: HashMap<String, String>,
    editor: Option<Editor>,
}

/// What links code in one conversation: its repository's forge and checkout, and the editor.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Links<'a> {
    forge: Option<&'a str>,
    checkout: Option<&'a str>,
    editor: Option<Editor>,
}

impl Known {
    /// Notes `web` as the forge of `channel`'s repository. Whoever speaks in the channel
    /// can name one, and every file, commit and branch said there then links to it: so once
    /// one is known, only an integration (CI, `trusted`) moves it to another host.
    pub(super) fn learn_forge(&mut self, channel: &str, web: &str, trusted: bool) {
        let Some(repo) = repo_of(channel) else {
            return;
        };
        let web = web.trim_end_matches('/');
        if !(web.starts_with("https://") || web.starts_with("http://")) {
            return;
        }
        let moved = self
            .forges
            .get(repo)
            .is_some_and(|known| host_of(known) != host_of(web));
        if !moved || trusted {
            self.forges.insert(repo.to_owned(), web.to_owned());
        }
    }

    pub(super) fn links(&self, channel: &str) -> Links<'_> {
        let repo = repo_of(channel);
        Links {
            forge: repo
                .and_then(|repo| self.forges.get(repo))
                .map(String::as_str),
            checkout: repo
                .and_then(|repo| self.checkouts.get(repo))
                .map(String::as_str),
            editor: self.editor,
        }
    }
}

impl super::Chat {
    /// Has each repository's files link into its checkout on this machine: `(channel,
    /// root)`, a repository's channel, `#api`, and its working tree's root, as zircd's
    /// repositories say. Apps with no checkouts (web, phones) never call it.
    pub fn set_checkouts(&mut self, checkouts: Vec<(String, String)>) {
        self.code.checkouts = checkouts
            .into_iter()
            .filter_map(|(channel, root)| {
                let repo = repo_of(&channel)?.to_owned();
                Some((repo, root.trim_end_matches('/').to_owned()))
            })
            .collect();
    }

    /// Has files in a checkout open in `editor`, or, with `None`, link to their forge.
    pub fn set_editor(&mut self, editor: Option<Editor>) {
        self.code.editor = editor;
    }

    /// Notes the repository's forge that `event` names, `repo` on a commit, a push or CI's
    /// landing, said in `channel`.
    pub(super) fn learn_forge(&mut self, channel: &str, from: &str, event: &zirc_proto::Event) {
        use zirc_proto::Event;
        let web = match event {
            Event::Commit {
                repo: Some(repo), ..
            }
            | Event::Push {
                repo: Some(repo), ..
            } => repo.as_str(),
            Event::Custom { data, .. } => {
                let Some(repo) = data.get("repo").and_then(|repo| repo.as_str()) else {
                    return;
                };
                repo
            }
            _ => return,
        };
        let trusted = zirc_proto::names::is_integration_nick(&from.to_ascii_lowercase());
        self.code.learn_forge(channel, web, trusted);
    }
}

/// The host of a forge's address, `https://git.example/team/repo`: `git.example`.
fn host_of(web: &str) -> &str {
    let rest = web.split_once("://").map_or(web, |(_, rest)| rest);
    rest.split('/').next().unwrap_or(rest)
}

/// The repository a channel is about: `monorepo` for `#monorepo` and `#monorepo/agents`.
fn repo_of(channel: &str) -> Option<&str> {
    let name = channel.strip_prefix('#')?;
    let repo = name.split('/').next().unwrap_or(name);
    (!repo.is_empty()).then_some(repo)
}

/// Code a word names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Found {
    /// A file in the repository, at a line and column.
    File {
        path: String,
        line: Option<u32>,
        column: Option<u32>,
    },
    /// A commit.
    Commit(String),
}

impl Links<'_> {
    /// Whether anything links here: a forge or a checkout.
    pub(super) fn any(&self) -> bool {
        self.forge.is_some() || self.checkout.is_some()
    }

    /// What `found` links to and the code it names, or `None` when nothing here links it.
    pub(super) fn resolve(&self, found: &Found) -> Option<(Option<String>, CodeRef)> {
        match found {
            Found::Commit(sha) => {
                let forge = self.forge?;
                let page = super::commit_link(forge, sha);
                Some((
                    Some(page.clone()),
                    CodeRef {
                        path: None,
                        line: None,
                        column: None,
                        commit: Some(sha.clone()),
                        local: None,
                        raw: None,
                        page: Some(page),
                    },
                ))
            }
            Found::File { path, line, column } => {
                let path = self.relative(path)?;
                let local = self.checkout.map(|root| format!("{root}/{path}"));
                let page = self.forge.map(|forge| file_page(forge, &path, *line));
                let raw = self.forge.map(|forge| raw_file(forge, &path));
                let url = match (&local, self.editor) {
                    (Some(local), Some(editor)) => Some(editor.link(local, *line, *column)),
                    _ => page.clone(),
                };
                if url.is_none() && local.is_none() {
                    return None;
                }
                Some((
                    url,
                    CodeRef {
                        path: Some(path),
                        line: *line,
                        column: *column,
                        commit: None,
                        local,
                        raw,
                        page,
                    },
                ))
            }
        }
    }

    /// `path` from the repository's root: as written, without a leading `./`, or an
    /// absolute path inside this machine's checkout. `None` for one outside it.
    fn relative(&self, path: &str) -> Option<String> {
        if path.starts_with('/') {
            let root = self.checkout?;
            return path
                .strip_prefix(root)
                .and_then(|rest| rest.strip_prefix('/'))
                .filter(|rest| !rest.is_empty())
                .map(str::to_owned);
        }
        Some(path.strip_prefix("./").unwrap_or(path).to_owned())
    }
}

/// The file's page on the forge, on its default branch: `<repo>/blob/HEAD/<path>#L42`,
/// `/-/blob/` on GitLab.
#[must_use]
pub fn file_page(repo: &str, path: &str, line: Option<u32>) -> String {
    let repo = repo.trim_end_matches('/');
    let anchor = line.map(|line| format!("#L{line}")).unwrap_or_default();
    let gitlab = if is_gitlab(repo) { "-/" } else { "" };
    format!("{repo}/{gitlab}blob/HEAD/{}{anchor}", escape_path(path))
}

/// The file's text on the forge, on its default branch, for a preview: GitHub's
/// `raw.githubusercontent.com`, GitLab's `/-/raw/`, others' `/raw/`.
#[must_use]
pub fn raw_file(repo: &str, path: &str) -> String {
    let repo = repo.trim_end_matches('/');
    let path = escape_path(path);
    if let Some(rest) = repo.strip_prefix("https://github.com/") {
        return format!("https://raw.githubusercontent.com/{rest}/HEAD/{path}");
    }
    if is_gitlab(repo) {
        return format!("{repo}/-/raw/HEAD/{path}");
    }
    format!("{repo}/raw/HEAD/{path}")
}

fn is_gitlab(repo: &str) -> bool {
    let host = repo
        .split_once("://")
        .map_or(repo, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or_default();
    host == "gitlab.com" || host.starts_with("gitlab.")
}

/// `path` safe in an address: spaces and the characters that end a path escaped.
fn escape_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '#' => out.push_str("%23"),
            '?' => out.push_str("%3F"),
            '%' => out.push_str("%25"),
            c => out.push(c),
        }
    }
    out
}

/// Code a whole word names, as a code span's text is: a file or a commit.
#[must_use]
pub(super) fn found(word: &str) -> Option<Found> {
    file(word).or_else(|| commit(word).map(|sha| Found::Commit(sha.to_owned())))
}

/// Each piece of plain `text` that names code, with where it is: `(start, end, found)`.
pub(super) fn find(text: &str) -> Vec<(usize, usize, Found)> {
    let mut out = Vec::new();
    let mut at = 0;
    for word in text.split_inclusive(char::is_whitespace) {
        let start = at;
        at += word.len();
        let word = word.trim_end();
        let (lead, core) = trim_punctuation(word);
        if core.is_empty() {
            continue;
        }
        if let Some(found) = found(core) {
            let begin = start + lead;
            out.push((begin, begin + core.len(), found));
        }
    }
    out
}

/// `word` without the punctuation prose puts around it: brackets and quotes before, and
/// those and a sentence's stops after. The bytes cut from its start, and what's left.
fn trim_punctuation(word: &str) -> (usize, &str) {
    let core = word.trim_start_matches(['(', '[', '{', '"', '\'', '<', '*', '_']);
    let lead = word.len() - core.len();
    let core = core.trim_end_matches([
        '.', ',', ';', ':', '!', '?', ')', ']', '}', '"', '\'', '>', '*', '_',
    ]);
    (lead, core)
}

/// A commit: 7 to 40 lowercase hex digits, a digit and a letter among them, so neither a
/// word such as `defaced` nor a number such as `2026111` reads as one.
fn commit(word: &str) -> Option<&str> {
    let ok = (7..=40).contains(&word.len())
        && word.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        && word.bytes().any(|b| b.is_ascii_digit())
        && word.bytes().any(|b| b.is_ascii_alphabetic());
    ok.then_some(word)
}

/// File names with no extension that are still clearly files.
const BARE_FILES: &[&str] = &[
    "BUILD",
    "Makefile",
    "Dockerfile",
    "Gemfile",
    "Rakefile",
    "Procfile",
    "Justfile",
    "WORKSPACE",
    "LICENSE",
];

/// A file in a repository, as people write one: `src/chat.rs`, `./web/app.tsx:12`,
/// `chat.rs:42:7` or `src/chat.rs#L42`.
fn file(word: &str) -> Option<Found> {
    if word.contains("://") || word.starts_with("www.") || word.starts_with('~') {
        return None;
    }
    let (path, line, column) = split_position(word);
    let slash = path.contains('/');
    if !slash && line.is_none() {
        return None;
    }
    if !path
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-' | b'+' | b'@'))
    {
        return None;
    }
    let inner = path.strip_prefix('/').unwrap_or(path);
    let inner = inner.strip_prefix("./").unwrap_or(inner);
    let segments: Vec<&str> = inner.split('/').collect();
    if segments
        .iter()
        .any(|segment| segment.is_empty() || segment.bytes().all(|b| b == b'.'))
    {
        return None;
    }
    let first = segments[0];
    // `zirc.dev/app.js` is an address without its scheme, not a file.
    if slash && !path.starts_with('/') && first.contains('.') && !first.starts_with('.') {
        return None;
    }
    // Something like `1/2` or `and/or` is prose, and a path's directories are words.
    if segments
        .iter()
        .any(|segment| !segment.bytes().any(|b| b.is_ascii_alphabetic()))
    {
        return None;
    }
    let name = segments.last().copied().unwrap_or_default();
    if !file_name(name) {
        return None;
    }
    Some(Found::File {
        path: path.to_owned(),
        line,
        column,
    })
}

/// Whether `name` is a file's: a stem and a lettered extension, `chat.rs`; a dotfile,
/// `.gitignore`; or one of [`BARE_FILES`].
fn file_name(name: &str) -> bool {
    if BARE_FILES.contains(&name) {
        return true;
    }
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    let extension_ok = (1..=10).contains(&extension.len())
        && extension.starts_with(|c: char| c.is_ascii_alphabetic())
        && extension.bytes().all(|b| b.is_ascii_alphanumeric());
    if !extension_ok {
        return false;
    }
    // `.gitignore`: a dotfile, its name all extension.
    stem.is_empty() || stem.bytes().any(|b| b.is_ascii_alphanumeric())
}

/// `word` without its position, and the position: `:42`, `:42:7` or `#L42`.
fn split_position(word: &str) -> (&str, Option<u32>, Option<u32>) {
    let number = |text: &str| -> Option<u32> {
        (!text.is_empty() && text.len() <= 7 && text.bytes().all(|b| b.is_ascii_digit()))
            .then(|| text.parse().ok())
            .flatten()
            .filter(|n| *n > 0)
    };
    if let Some((path, anchor)) = word.rsplit_once("#L")
        && let Some(line) = number(anchor)
    {
        return (path, Some(line), None);
    }
    if let Some((rest, last)) = word.rsplit_once(':')
        && let Some(last) = number(last)
    {
        if let Some((path, line)) = rest.rsplit_once(':')
            && let Some(line) = number(line)
        {
            return (path, Some(line), Some(last));
        }
        return (rest, Some(last), None);
    }
    (word, None, None)
}

/// `text` with each stack trace and diff pasted without a fence fenced, so it reads as a
/// code block: cut when long, and a diff coloured. Text with neither comes back as it is.
#[must_use]
pub(super) fn fence_pasted(text: &str) -> Cow<'_, str> {
    // Most messages have none of what every trace or diff has: said at once.
    if !text.contains('\n') || !PASTED_MARKS.iter().any(|mark| text.contains(mark)) {
        return Cow::Borrowed(text);
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let mut runs = Vec::new();
    let mut fenced = false;
    let mut at = 0;
    while at < lines.len() {
        let line = lines[at];
        if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
            fenced = !fenced;
            at += 1;
            continue;
        }
        if !fenced && let Some((end, language)) = pasted_run(&lines, at) {
            runs.push((at, end, language));
            at = end;
            continue;
        }
        at += 1;
    }
    if runs.is_empty() {
        return Cow::Borrowed(text);
    }
    let mut out = Vec::with_capacity(lines.len() + runs.len() * 2);
    let mut at = 0;
    for (start, end, language) in runs {
        out.extend_from_slice(&lines[at..start]);
        out.push(if language.is_empty() {
            "```"
        } else {
            "```diff"
        });
        out.extend_from_slice(&lines[start..end]);
        out.push("```");
        at = end;
    }
    out.extend_from_slice(&lines[at..]);
    Cow::Owned(out.join("\n"))
}

/// What every trace or diff [`fence_pasted`] fences has somewhere: a hunk, a frame's `at `
/// or `File "`, Rust's numbered frames' `::`, Go's ` +0x`.
const PASTED_MARKS: &[&str] = &["@@ -", " at ", "\tat ", "File \"", "::", " +0x"];

/// A trace or a diff starting at `lines[start]`: where it ends, and its fence's language,
/// `diff` or none.
fn pasted_run(lines: &[&str], start: usize) -> Option<(usize, &'static str)> {
    diff_run(lines, start)
        .map(|end| (end, "diff"))
        .or_else(|| trace_run(lines, start).map(|end| (end, "")))
}

/// A diff: its header, `diff --git` or `--- a/…`, or a hunk's `@@ -1,2 +1,3 @@`, then
/// lines a diff has, at least one hunk among them.
fn diff_run(lines: &[&str], start: usize) -> Option<usize> {
    let first = lines[start];
    if !(first.starts_with("diff --git ") || first.starts_with("--- ") || is_hunk(first)) {
        return None;
    }
    let mut end = start;
    let mut hunks = 0;
    while end < lines.len() {
        let line = lines[end];
        let diffish = is_hunk(line)
            || [
                "diff ",
                "index ",
                "--- ",
                "+++ ",
                "new file",
                "deleted file",
                "similarity ",
            ]
            .iter()
            .any(|lead| line.starts_with(lead))
            || line.starts_with("rename ")
            || line.starts_with("\\ No newline")
            || line.starts_with([' ', '+', '-']);
        if !diffish {
            break;
        }
        if is_hunk(line) {
            hunks += 1;
        }
        end += 1;
    }
    (hunks > 0 && end - start >= 2).then_some(end)
}

/// `@@ -12,7 +12,8 @@`, with anything after it.
fn is_hunk(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("@@ -") else {
        return false;
    };
    let range = |text: &str| {
        let mut parts = text.splitn(2, ',');
        parts
            .next()
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            && parts
                .next()
                .is_none_or(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    };
    let mut words = rest.splitn(3, ' ');
    let old = words.next().unwrap_or_default();
    let new = words.next().unwrap_or_default();
    let close = words.next().unwrap_or_default();
    range(old) && new.strip_prefix('+').is_some_and(range) && close.starts_with("@@")
}

/// A stack trace: lines that name frames, three at least, with the lines between them a
/// trace has (Python's source lines, a header such as `Traceback (most recent call last):`,
/// the error after it). Rust's, Python's, JavaScript's, Java's and Go's.
fn trace_run(lines: &[&str], start: usize) -> Option<usize> {
    let first = lines[start];
    if !(is_trace_header(first) || is_error_line(first) || is_frame(first)) {
        return None;
    }
    let mut frames = 0;
    let mut header = false;
    let mut end = start;
    let mut last_frame = start;
    while end < lines.len() {
        let line = lines[end];
        if is_frame(line) {
            frames += 1;
            last_frame = end;
        } else if is_trace_header(line) {
            header = true;
        } else if !((end == start && is_error_line(line))
            || (line.starts_with([' ', '\t']) && !line.trim().is_empty()))
        {
            break;
        }
        end += 1;
    }
    if frames < 3 && !(header && frames > 0) {
        return None;
    }
    // Indented lines after the last frame are its source, as Python shows; and the line
    // after a trace is usually its error, `ValueError: bad`.
    let mut end = end.max(last_frame + 1);
    if let Some(next) = lines.get(end)
        && is_error_line(next)
    {
        end += 1;
    }
    Some(end)
}

fn is_trace_header(line: &str) -> bool {
    let line = line.trim();
    line == "Traceback (most recent call last):"
        || line == "stack backtrace:"
        || (line.starts_with("thread '") && line.contains("panicked at"))
        || (line.starts_with("goroutine ") && line.ends_with("]:"))
        || line.starts_with("Exception in thread ")
        || line.starts_with("Caused by:")
        || (line.starts_with("...") && line.ends_with(" more"))
}

/// A frame: `at f (file.js:1:2)`, `File "x.py", line 3, in f`, Rust's `  12: name` and
/// `at ./src/main.rs:5:5`, Go's `/src/main.go:42 +0x1d`.
fn is_frame(line: &str) -> bool {
    let indented = line.starts_with([' ', '\t']);
    let trimmed = line.trim();
    if indented && let Some(rest) = trimmed.strip_prefix("at ") {
        return has_position(rest);
    }
    if trimmed.starts_with("File \"") && trimmed.contains("\", line ") {
        return true;
    }
    if indented
        && let Some((number, rest)) = trimmed.split_once(": ")
        && !number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit())
        && (rest.contains("::") || rest.starts_with("0x") || rest.starts_with('<'))
    {
        return true;
    }
    if let Some((place, offset)) = trimmed.rsplit_once(" +0x") {
        return place.contains(".go:") && offset.bytes().all(|b| b.is_ascii_hexdigit());
    }
    false
}

/// Whether `text` has a `:` and a digit after it, as a frame's file and line do.
fn has_position(text: &str) -> bool {
    text.match_indices(':')
        .any(|(at, _)| text[at + 1..].starts_with(|c: char| c.is_ascii_digit()))
}

/// A trace's error line: `ValueError: bad`, `java.lang.IllegalStateException: no`,
/// `Error: boom`.
fn is_error_line(line: &str) -> bool {
    let Some((name, _)) = line.split_once(':') else {
        return false;
    };
    !name.is_empty()
        && !name.contains(' ')
        && (name.ends_with("Error") || name.ends_with("Exception"))
}

/// The language a fence that names none is in, when its code is clearly a diff: `diff`.
pub(super) fn sniff_language(code: &str) -> Option<&'static str> {
    let lines: Vec<&str> = code.lines().collect();
    (0..lines.len())
        .any(|start| diff_run(&lines, start).is_some())
        .then_some("diff")
}

/// How many lines a preview card shows above the line it names, and below it.
pub const PREVIEW_AROUND: u32 = 3;

/// A file's few lines around the one a message names, highlighted, as a preview card shows
/// them on a hover or a long press.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CodePreview {
    /// Its title: the file's path and the line, `src/chat.rs:42`.
    pub title: String,
    /// The number of its first line, from 1.
    pub first: u32,
    /// Which of its lines is the one named, from 0: the app marks it. `None` when no line
    /// was named.
    pub focus: Option<u32>,
    /// The language its lines are in, by the file's extension, `rs`, for an app that
    /// highlights them itself.
    pub language: Option<String>,
    /// Its lines' text.
    pub text: String,
    /// Its lines, highlighted, when a highlighter was given ([`code_preview`]); empty
    /// otherwise, for the app to draw or colour `text` itself.
    pub lines: Vec<super::CodeLine>,
    /// Each line's number, as its gutter shows it, right-aligned to the widest.
    pub numbers: Vec<String>,
    /// What a screen reader says of it: `src/chat.rs, lines 39 to 45`.
    pub spoken: String,
    /// Why it shows no lines, when it can't: the file isn't checked out here and its forge
    /// didn't give it, as for a private repository's.
    pub note: Option<String>,
    /// Where it opens, the editor first ([`code_actions`]): its buttons.
    pub actions: Vec<CodeAction>,
}

/// Somewhere a preview card opens its file: the editor, or the forge's page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CodeAction {
    /// What its button says: `Open in VS Code`, `Open on github.com`.
    pub label: String,
    /// The address it opens.
    pub url: String,
}

/// What a preview card says when it has no lines to show.
pub const NO_PREVIEW: &str = "No preview: it isn't checked out here, and its forge didn't share it";

/// Where the file `code` names opens, as a preview card's buttons: in `editor` where it's
/// checked out here, then on its forge.
#[must_use]
pub fn code_actions(code: &CodeRef, editor: Option<Editor>) -> Vec<CodeAction> {
    let mut actions = Vec::new();
    if let (Some(local), Some(editor)) = (&code.local, editor) {
        actions.push(CodeAction {
            label: super::labels::open_in_label(editor.name()),
            url: editor.link(local, code.line, code.column),
        });
    }
    if let Some(page) = &code.page {
        let host = page
            .split_once("://")
            .map_or(page.as_str(), |(_, rest)| rest)
            .split('/')
            .next()
            .unwrap_or_default();
        actions.push(CodeAction {
            label: format!("Open on {host}"),
            url: page.clone(),
        });
    }
    actions
}

impl CodePreview {
    /// The card for the file `code` names when its text can't be had: its title, why, and
    /// where it opens.
    #[must_use]
    pub fn unavailable(code: &CodeRef, editor: Option<Editor>) -> Self {
        let path = code.path.clone().unwrap_or_default();
        Self {
            title: match code.line {
                Some(line) => format!("{path}:{line}"),
                None => path.clone(),
            },
            first: 1,
            focus: None,
            language: None,
            text: String::new(),
            lines: Vec::new(),
            numbers: Vec::new(),
            spoken: path,
            note: Some(NO_PREVIEW.to_owned()),
            actions: code_actions(code, editor),
        }
    }
}

/// The preview of the file `code` names, from `text`, the file's text: [`PREVIEW_AROUND`]
/// lines either side of its line, or its first lines when it names none. Its lines come
/// highlighted by `highlighter` ([`super::highlighted_lines`] for zirc-chat's own), or, with
/// none, are left for the app to colour from its text, as the web client does. `None` for a
/// commit, or a line past the file's end.
#[must_use]
pub fn code_preview(
    code: &CodeRef,
    text: &str,
    highlighter: Option<super::Highlighter>,
) -> Option<CodePreview> {
    let path = code.path.as_deref()?;
    let lines: Vec<&str> = text.lines().collect();
    let total = u32::try_from(lines.len()).unwrap_or(u32::MAX);
    if total == 0 {
        return None;
    }
    let shown = PREVIEW_AROUND * 2 + 1;
    let (first, focus) = match code.line {
        Some(line) if line > total => return None,
        Some(line) => {
            let first = line.saturating_sub(PREVIEW_AROUND).max(1);
            let first = first.min(total.saturating_sub(shown - 1).max(1));
            (first, Some(line - first))
        }
        None => (1, None),
    };
    let last = (first + shown - 1).min(total);
    let start = usize::try_from(first - 1).unwrap_or_default();
    let end = usize::try_from(last).unwrap_or(lines.len());
    let snippet = lines[start..end].join("\n");
    let extension = path.rsplit_once('.').map(|(_, extension)| extension);
    let width = last.to_string().len();
    let title = match code.line {
        Some(line) => format!("{path}:{line}"),
        None => path.to_owned(),
    };
    Some(CodePreview {
        title,
        first,
        focus,
        lines: highlighter
            .map(|highlight| highlight(extension, &snippet))
            .unwrap_or_default(),
        language: extension.map(str::to_owned),
        text: snippet,
        numbers: (first..=last).map(|n| format!("{n:>width$}")).collect(),
        spoken: format!("{path}, lines {first} to {last}"),
        note: None,
        actions: Vec::new(),
    })
}

#[cfg(test)]
mod tests;
