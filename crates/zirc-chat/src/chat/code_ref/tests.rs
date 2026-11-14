use super::*;
use crate::chat::{Chat, Options, RichBlock, RichSpanKind, rich};

fn file_at(path: &str, line: Option<u32>, column: Option<u32>) -> Found {
    Found::File {
        path: path.to_owned(),
        line,
        column,
    }
}

/// What `find` names in `text`, as the words it found.
fn named(text: &str) -> Vec<&str> {
    find(text)
        .into_iter()
        .map(|(start, end, _)| &text[start..end])
        .collect()
}

#[test]
fn finds_paths_with_and_without_positions() {
    assert_eq!(
        found("oss/crates/zirc-chat/src/chat.rs:42"),
        Some(file_at("oss/crates/zirc-chat/src/chat.rs", Some(42), None))
    );
    assert_eq!(
        found("chat.rs:42:7"),
        Some(file_at("chat.rs", Some(42), Some(7)))
    );
    assert_eq!(
        found("./web/src/App.tsx"),
        Some(file_at("./web/src/App.tsx", None, None))
    );
    assert_eq!(
        found("src/chat.rs#L12"),
        Some(file_at("src/chat.rs", Some(12), None))
    );
    assert_eq!(
        found(".github/workflows/ci.yml"),
        Some(file_at(".github/workflows/ci.yml", None, None))
    );
    assert_eq!(
        found("oss/apps/BUILD"),
        Some(file_at("oss/apps/BUILD", None, None))
    );
    assert_eq!(
        found("a/.gitignore"),
        Some(file_at("a/.gitignore", None, None))
    );
    assert_eq!(
        found("/Users/me/src/api/main.go:3"),
        Some(file_at("/Users/me/src/api/main.go", Some(3), None))
    );
}

#[test]
fn leaves_prose_versions_and_addresses_alone() {
    for word in [
        "1.2.3",
        "v1.2.3",
        "1.2.3:4",
        "and/or",
        "e.g.",
        "i.e",
        "README.md",
        "chat.rs",
        "Node.js",
        "zirc.dev",
        "zirc.dev/app.js",
        "github.com/zircapp/zirc/blob/main/x.rs",
        "https://github.com/acme/api/blob/main/src/a.rs",
        "1/2",
        "50/50",
        "km/h",
        "10:30",
        "12:30:45",
        "TCP/IP",
        "src/",
        "oss/crates",
        "../up/a.rs",
        "a//b.rs",
        "~/notes/today.md",
        "src/a.rs:0",
        "src/a.rs:",
        "a/b.123",
        "build/2026/x.rs",
        "x:12",
    ] {
        assert_eq!(found(word), None, "{word}");
    }
}

#[test]
fn finds_commits_strictly() {
    assert_eq!(found("3f2a1c0"), Some(Found::Commit("3f2a1c0".into())));
    let full = "67606a6cd1e9f0a8b7c6d5e4f3a2b1c0d9e8f7a6";
    assert_eq!(found(full), Some(Found::Commit(full.into())));
    for word in [
        "defaced",                                   // a word, all hex letters
        "2026111",                                   // a number
        "3f2a1c",                                    // too short
        "3F2A1C0",                                   // uppercase
        "0x3f2a1c0",                                 // a hex literal
        "#3f2a1c0",                                  // a colour, or an issue
        "3f2a1c0g",                                  // not hex
        "67606a6cd1e9f0a8b7c6d5e4f3a2b1c0d9e8f7a6a", // too long
        "abc1234..def5678",
    ] {
        assert_eq!(found(word), None, "{word}");
    }
}

#[test]
fn finds_code_in_sentences_without_their_punctuation() {
    assert_eq!(
        named("see oss/crates/zirc-chat/src/chat.rs:42, then (web/src/App.tsx) and 3f2a1c0."),
        [
            "oss/crates/zirc-chat/src/chat.rs:42",
            "web/src/App.tsx",
            "3f2a1c0"
        ]
    );
    assert_eq!(
        named("landed in \"src/lib.rs:7\"; reverted by abc1234!"),
        ["src/lib.rs:7", "abc1234"]
    );
    assert!(named("version 1.2.3 is out, and/or 2.0 e.g. on zirc.dev").is_empty());
    assert!(named("a uuid 550e8400-e29b-41d4-a716-446655440000 and build-abc1234").is_empty());
    assert!(named("we decaffed 1234567 cups at 10:30").is_empty());
}

fn chat_in(forge: Option<&str>, checkout: Option<&str>, editor: Option<Editor>) -> Chat {
    let mut chat = Chat::new("me", "irc.zirc.test", Vec::new(), Options::default());
    if let Some(forge) = forge {
        chat.code.learn_forge("#api", forge, false);
    }
    if let Some(checkout) = checkout {
        chat.set_checkouts(vec![("#api".into(), checkout.into())]);
    }
    chat.set_editor(editor);
    chat
}

/// The links and code `text` has when said in `channel`.
fn links(chat: &Chat, channel: &str, text: &str) -> Vec<(String, Option<String>, Option<CodeRef>)> {
    let mut out = Vec::new();
    RichBlock::each_span(&chat.flat_in(channel, rich(text)), &mut |span| {
        let url = match &span.kind {
            RichSpanKind::Link { url } => Some(url.clone()),
            _ => None,
        };
        if url.is_some() || span.code.is_some() {
            out.push((span.text.clone(), url, span.code.clone()));
        }
    });
    out
}

#[test]
fn links_files_and_commits_to_the_forge_where_there_is_no_checkout() {
    let chat = chat_in(
        Some("https://github.com/acme/api"),
        None,
        Some(Editor::VsCode),
    );
    let found = links(&chat, "#api/agents", "see `src/a.rs:4` and 3f2a1c0");
    assert_eq!(found.len(), 2, "{found:?}");
    let (text, url, code) = &found[0];
    assert_eq!(text, "src/a.rs:4");
    assert_eq!(
        url.as_deref(),
        Some("https://github.com/acme/api/blob/HEAD/src/a.rs#L4")
    );
    let code = code.as_ref().unwrap();
    assert_eq!(code.path.as_deref(), Some("src/a.rs"));
    assert_eq!(code.line, Some(4));
    assert_eq!(code.local, None);
    assert_eq!(
        code.raw.as_deref(),
        Some("https://raw.githubusercontent.com/acme/api/HEAD/src/a.rs")
    );
    let (text, url, code) = &found[1];
    assert_eq!(text, "3f2a1c0");
    assert_eq!(
        url.as_deref(),
        Some("https://github.com/acme/api/commit/3f2a1c0")
    );
    assert_eq!(code.as_ref().unwrap().commit.as_deref(), Some("3f2a1c0"));
}

#[test]
fn links_files_into_the_editor_where_the_repository_is_checked_out() {
    let chat = chat_in(
        Some("https://gitlab.com/acme/api"),
        Some("/Users/me/src/api/"),
        Some(Editor::Zed),
    );
    let found = links(
        &chat,
        "#api",
        "broke at src/a.rs:4:2 and /Users/me/src/api/web/x.ts:9, not /etc/hosts.conf:1",
    );
    let urls: Vec<_> = found.iter().map(|(_, url, _)| url.as_deref()).collect();
    assert_eq!(
        urls,
        [
            Some("zed://file/Users/me/src/api/src/a.rs:4:2"),
            Some("zed://file/Users/me/src/api/web/x.ts:9"),
        ]
    );
    let code = found[1].2.as_ref().unwrap();
    assert_eq!(code.path.as_deref(), Some("web/x.ts"));
    assert_eq!(code.local.as_deref(), Some("/Users/me/src/api/web/x.ts"));
    assert_eq!(
        code.page.as_deref(),
        Some("https://gitlab.com/acme/api/-/blob/HEAD/web/x.ts#L9")
    );
    assert_eq!(
        code.raw.as_deref(),
        Some("https://gitlab.com/acme/api/-/raw/HEAD/web/x.ts")
    );
    // A checkout with no forge known still opens files, but can't link a commit.
    let chat = chat_in(None, Some("/src/api"), Some(Editor::VsCode));
    let found = links(&chat, "#api", "src/a.rs:4 in 3f2a1c0");
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].1.as_deref(),
        Some("vscode://file/src/api/src/a.rs:4")
    );
}

#[test]
fn links_nothing_in_a_channel_of_no_known_repository() {
    let chat = chat_in(Some("https://github.com/acme/api"), None, None);
    assert!(links(&chat, "#general", "src/a.rs:4 and 3f2a1c0").is_empty());
    assert!(links(&chat, "bob", "src/a.rs:4 and 3f2a1c0").is_empty());
    // Nor in links, mentions or code blocks, nor inline code that's more than one.
    let found = links(
        &chat,
        "#api",
        "https://example.com/3f2a1c0/src/a.rs:4 `src/a.rs:4 3f2a1c0`\n\n```\nsrc/a.rs:4\n```",
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "https://example.com/3f2a1c0/src/a.rs:4");
}

#[test]
fn keeps_styles_and_the_words_around_code() {
    let chat = chat_in(Some("https://github.com/acme/api"), None, None);
    let blocks = chat.flat_in("#api", rich("**fixed in src/a.rs:4 today**"));
    let [RichBlock::Paragraph { spans }] = blocks.as_slice() else {
        panic!("{blocks:?}");
    };
    let texts: Vec<&str> = spans.iter().map(|span| span.text.as_str()).collect();
    assert_eq!(texts, ["fixed in ", "src/a.rs:4", " today"]);
    assert!(spans.iter().all(|span| span.bold));
}

#[test]
fn learns_each_repositorys_forge_from_what_is_said() {
    let mut chat = chat_in(None, None, None);
    let commit = |repo: &str| zirc_proto::Event::Commit {
        paths: Vec::new(),
        branch: "main".into(),
        sha: "3f2a1c0aa".into(),
        subject: "x".into(),
        repo: Some(repo.into()),
    };
    let github = commit("https://github.com/acme/api");
    chat.learn_forge("#api/agents", "ada-mbp", &github);
    assert_eq!(links(&chat, "#api", "3f2a1c0").len(), 1);
    assert!(links(&chat, "#web", "3f2a1c0").is_empty());
    // Known, it isn't moved to another host by whoever says so: only by an integration.
    let forge = |chat: &Chat| chat.code.links("#api").forge.map(str::to_owned);
    chat.learn_forge("#api", "bob-mbp", &commit("https://evil.example/x"));
    assert_eq!(forge(&chat).unwrap(), "https://github.com/acme/api");
    chat.learn_forge("#api", "bob-mbp", &commit("https://github.com/acme/api2"));
    assert_eq!(forge(&chat).unwrap(), "https://github.com/acme/api2");
    chat.learn_forge("#api", "zirc-ci", &commit("https://git.acme.example/api"));
    assert_eq!(forge(&chat).unwrap(), "https://git.acme.example/api");
}

#[test]
fn makes_each_editors_address() {
    let file = "/Users/me/my code/a.rs";
    assert_eq!(
        Editor::VsCode.link(file, Some(4), Some(2)),
        "vscode://file/Users/me/my%20code/a.rs:4:2"
    );
    assert_eq!(
        Editor::Cursor.link(file, Some(4), None),
        "cursor://file/Users/me/my%20code/a.rs:4"
    );
    assert_eq!(
        Editor::Zed.link(file, None, None),
        "zed://file/Users/me/my%20code/a.rs"
    );
    assert_eq!(
        Editor::Xcode.link(file, Some(4), None),
        "file:///Users/me/my%20code/a.rs"
    );
    for editor in Editor::ALL {
        assert_eq!(Editor::from_id(editor.id()), Some(editor));
    }
}

#[test]
fn fences_pasted_traces() {
    let python = "it broke:\nTraceback (most recent call last):\n  File \"a.py\", line 3, in <module>\n    main()\n  File \"a.py\", line 2, in main\n    raise ValueError(\"bad\")\nValueError: bad\nany idea?";
    assert_eq!(
        fence_pasted(python),
        "it broke:\n```\nTraceback (most recent call last):\n  File \"a.py\", line 3, in <module>\n    main()\n  File \"a.py\", line 2, in main\n    raise ValueError(\"bad\")\nValueError: bad\n```\nany idea?"
    );
    let js = "TypeError: x is undefined\n    at f (/app/a.js:1:2)\n    at g (/app/b.js:3:4)\n    at h (/app/c.js:5:6)";
    assert_eq!(fence_pasted(js), format!("```\n{js}\n```"));
    let rust = "stack backtrace:\n   0: std::panicking::begin_panic\n             at /rustc/abc/library/std/src/panicking.rs:5:5\n   1: app::main\n             at ./src/main.rs:5:5";
    assert_eq!(fence_pasted(rust), format!("```\n{rust}\n```"));
    let java = "Exception in thread \"main\" java.lang.IllegalStateException: no\n\tat com.acme.A.run(A.java:12)\n\tat com.acme.Main.main(Main.java:4)";
    assert_eq!(fence_pasted(java), format!("```\n{java}\n```"));
    let go = "goroutine 1 [running]:\nmain.main()\n\t/src/main.go:42 +0x1d\nexit status 2";
    // Go's function lines aren't indented: only the frame and what follows is a trace here,
    // and one frame with no header before it isn't enough.
    assert_eq!(fence_pasted(go), go);
}

#[test]
fn leaves_prose_that_looks_a_little_like_a_trace() {
    for text in [
        "Steps:\n  1: build\n  2: test\n  3: deploy",
        "we met\n    at noon\n    at the office\n    at last",
        "  at 10:30 we start\n  at 11:00 we stop",
        "Error: something went wrong\nplease retry",
        "one line",
        "```\n    at f (/a.js:1:2)\n    at g (/a.js:1:2)\n    at h (/a.js:1:2)\n```",
    ] {
        assert_eq!(fence_pasted(text), text, "{text}");
    }
}

#[test]
fn fences_pasted_diffs_and_names_unnamed_fences_diff() {
    let diff = "diff --git a/x.rs b/x.rs\nindex 1..2 100644\n--- a/x.rs\n+++ b/x.rs\n@@ -1,2 +1,2 @@\n fn a() {}\n-old\n+new";
    let pasted = format!("look:\n{diff}\nok?");
    assert_eq!(
        fence_pasted(&pasted),
        format!("look:\n```diff\n{diff}\n```\nok?")
    );
    let blocks = rich(&pasted);
    assert!(
        blocks.iter().any(|block| matches!(
            block,
            crate::chat::Block::Code { language: Some(language), .. } if language == "diff"
        )),
        "{blocks:?}"
    );
    // A fence that names nothing, around a diff.
    let blocks = rich("```\n@@ -1 +1 @@\n-a\n+b\n```");
    assert!(matches!(
        blocks.as_slice(),
        [crate::chat::Block::Code { language: Some(language), .. }] if language == "diff"
    ));
    // A list is not a diff, nor is a hunk-like line alone.
    for text in ["- one\n- two\n+ three", "@@ -1 +1 @@", "-- \nsignature"] {
        assert_eq!(fence_pasted(text), text, "{text}");
    }
}

#[test]
fn makes_forge_pages_and_raw_addresses() {
    assert_eq!(
        file_page("https://github.com/a/b/", "x y.rs", Some(3)),
        "https://github.com/a/b/blob/HEAD/x%20y.rs#L3"
    );
    assert_eq!(
        raw_file("https://codeberg.org/a/b", "x.rs"),
        "https://codeberg.org/a/b/raw/HEAD/x.rs"
    );
}

/// zirc-chat's own highlighter, as apps pass it.
const LIT: Option<crate::chat::Highlighter> = Some(crate::chat::highlighted_lines);

fn code(path: &str, line: Option<u32>) -> CodeRef {
    CodeRef {
        path: Some(path.into()),
        line,
        column: None,
        commit: None,
        local: None,
        raw: None,
        page: None,
    }
}

#[test]
fn previews_three_lines_either_side_highlighted() {
    let text = (1..=20)
        .map(|n| format!("let x{n} = {n};"))
        .collect::<Vec<_>>()
        .join("\n");
    let preview = code_preview(&code("src/a.rs", Some(10)), &text, LIT).unwrap();
    assert_eq!(preview.title, "src/a.rs:10");
    assert_eq!((preview.first, preview.focus), (7, Some(3)));
    assert_eq!(preview.lines.len(), 7);
    assert_eq!(preview.numbers.first().map(String::as_str), Some(" 7"));
    assert_eq!(preview.numbers.last().map(String::as_str), Some("13"));
    assert_eq!(preview.lines[0].tokens[0].role, "syntax-keyword");
    assert_eq!(preview.spoken, "src/a.rs, lines 7 to 13");
    // Near the start and the end, it keeps seven lines where the file has them.
    let start = code_preview(&code("src/a.rs", Some(2)), &text, LIT).unwrap();
    assert_eq!(
        (start.first, start.focus, start.lines.len()),
        (1, Some(1), 7)
    );
    let end = code_preview(&code("src/a.rs", Some(20)), &text, LIT).unwrap();
    assert_eq!((end.first, end.focus, end.lines.len()), (14, Some(6), 7));
    // No line: its first lines. Past the end, or a short file.
    let top = code_preview(&code("src/a.rs", None), &text, LIT).unwrap();
    assert_eq!(
        (top.first, top.focus, top.title.as_str()),
        (1, None, "src/a.rs")
    );
    assert_eq!(code_preview(&code("src/a.rs", Some(21)), &text, LIT), None);
    let short = code_preview(&code("a.py", Some(2)), "a = 1\nb = 2\n", LIT).unwrap();
    assert_eq!(
        (short.first, short.focus, short.lines.len()),
        (1, Some(1), 2)
    );
    assert_eq!(code_preview(&code("a.py", Some(1)), "", LIT), None);
}

#[test]
fn leaves_a_previews_colour_to_an_app_without_a_highlighter() {
    let preview = code_preview(
        &code("web/a.ts", Some(1)),
        "const a = 1;\nconst b = 2;",
        None,
    )
    .unwrap();
    assert!(preview.lines.is_empty());
    assert_eq!(preview.language.as_deref(), Some("ts"));
    assert_eq!(preview.text, "const a = 1;\nconst b = 2;");
}

#[test]
fn offers_the_editor_then_the_forge() {
    let file = CodeRef {
        local: Some("/src/api/a.rs".into()),
        page: Some("https://github.com/acme/api/blob/HEAD/a.rs#L4".into()),
        ..code("a.rs", Some(4))
    };
    let labels: Vec<(String, String)> = code_actions(&file, Some(Editor::Cursor))
        .into_iter()
        .map(|action| (action.label, action.url))
        .collect();
    assert_eq!(
        labels,
        [
            (
                "Open in Cursor".to_owned(),
                "cursor://file/src/api/a.rs:4".to_owned()
            ),
            (
                "Open on github.com".to_owned(),
                "https://github.com/acme/api/blob/HEAD/a.rs#L4".to_owned()
            ),
        ]
    );
    let none = CodePreview::unavailable(&file, None);
    assert_eq!(none.title, "a.rs:4");
    assert_eq!(none.note.as_deref(), Some(NO_PREVIEW));
    assert_eq!(none.actions.len(), 1);
}

#[test]
fn lists_where_code_opens_automatic_first_and_the_forge_last() {
    let installed = [Editor::Cursor, Editor::Xcode];
    let shown = |chosen| {
        editor_options(&installed, chosen)
            .into_iter()
            .map(|option| (option.label, option.chosen))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        shown(None),
        [
            ("Automatic (Cursor)".to_owned(), true),
            ("Cursor".to_owned(), false),
            ("Xcode".to_owned(), false),
            ("The forge".to_owned(), false),
        ]
    );
    // One chosen but no longer installed still shows, chosen.
    assert!(shown(Some("zed")).contains(&("Zed".to_owned(), true)));
    assert_eq!(editor_options(&[], None)[0].label, "Automatic (the forge)");
    assert_eq!(chosen_editor(None, &installed), Some(Editor::Cursor));
    assert_eq!(
        chosen_editor(Some(EDITOR_AUTOMATIC), &installed),
        Some(Editor::Cursor)
    );
    assert_eq!(chosen_editor(Some("zed"), &installed), Some(Editor::Zed));
    assert_eq!(chosen_editor(Some(EDITOR_FORGE), &installed), None);
}
