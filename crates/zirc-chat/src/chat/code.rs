//! A code block's text, highlighted: each line as tokens with a role, which zirc-theme
//! colours the same in every app, light and dark. The grammars are syntect's bundled
//! Sublime Text ones; its themes aren't used.

use std::sync::OnceLock;

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

/// What a piece of code is, for its colour.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Role {
    /// Anything else, in the code's own colour.
    #[default]
    Plain,
    /// A keyword, such as `fn` or `if`.
    Keyword,
    /// A string.
    String,
    /// A comment.
    Comment,
    /// A number, or a constant such as `true`.
    Number,
    /// A type's name.
    Type,
    /// A function's name.
    Function,
    /// A line a diff adds.
    Added,
    /// A line a diff removes.
    Removed,
}

impl Role {
    /// The zirc-theme role that colours it: `syntax-keyword` and the like, or `body` for
    /// plain code.
    #[must_use]
    pub fn theme_role(self) -> &'static str {
        match self {
            Self::Plain => "body",
            Self::Keyword => "syntax-keyword",
            Self::String => "syntax-string",
            Self::Comment => "syntax-comment",
            Self::Number => "syntax-number",
            Self::Type => "syntax-type",
            Self::Function => "syntax-function",
            Self::Added => "syntax-added",
            Self::Removed => "syntax-removed",
        }
    }
}

/// A run of code in one role, as apps take it, the web client in JSON: its role as
/// zirc-theme names it ([`Role::theme_role`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
pub struct Token {
    /// The code.
    pub text: String,
    /// Its role.
    #[serde(with = "theme_role")]
    #[ts(type = "string")]
    pub role: Role,
}

/// `role` as zirc-theme names it, for [`Token`]'s JSON.
mod theme_role {
    use super::Role;

    /// For `#[serde(with)]`, which ts-rs reads (it warns of `serialize_with`).
    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "serde passes a reference"
    )]
    pub fn serialize<S: serde::Serializer>(role: &Role, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(role.theme_role())
    }
}

/// `code`'s lines as tokens, highlighted as `language` (a fence's name, such as `rust`,
/// `ts` or `diff`). Unknown languages, and none, come back as plain lines.
#[must_use]
pub fn highlight(code: &str, language: Option<&str>) -> Vec<Vec<Token>> {
    let plain = || {
        code.lines()
            .map(|line| {
                vec![Token {
                    text: line.to_owned(),
                    role: Role::Plain,
                }]
            })
            .collect()
    };
    let syntaxes = syntaxes();
    let Some(syntax) = language.and_then(|language| syntax(syntaxes, language)) else {
        return plain();
    };
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut lines = Vec::new();
    for line in LinesWithEndings::from(code) {
        let Ok(ops) = state.parse_line(line, syntaxes) else {
            return plain();
        };
        let mut tokens: Vec<Token> = Vec::new();
        let mut at = 0;
        let mut add = |text: &str, role: Role| {
            let text = text.trim_end_matches(['\n', '\r']);
            if text.is_empty() {
                return;
            }
            match tokens.last_mut() {
                Some(last) if last.role == role => last.text.push_str(text),
                _ => tokens.push(Token {
                    text: text.to_owned(),
                    role,
                }),
            }
        };
        for (end, op) in ops {
            add(&line[at..end], role(&stack));
            at = end;
            if stack.apply(&op).is_err() {
                return plain();
            }
        }
        add(&line[at..], role(&stack));
        lines.push(tokens);
    }
    lines
}

/// syntect's bundled grammars, loaded once.
fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// The grammar a fence's `language` names, by its name or a file extension.
fn syntax<'a>(syntaxes: &'a SyntaxSet, language: &str) -> Option<&'a SyntaxReference> {
    let language = match language.to_ascii_lowercase().as_str() {
        // No TypeScript grammar is bundled: JavaScript's is close.
        "ts" | "typescript" | "tsx" | "jsx" => "js".to_owned(),
        "shell" | "zsh" | "console" => "sh".to_owned(),
        "golang" => "go".to_owned(),
        other => other.to_owned(),
    };
    syntaxes
        .find_syntax_by_token(&language)
        .or_else(|| syntaxes.find_syntax_by_extension(&language))
}

/// The role of code in `stack`'s scopes: the innermost that has one.
fn role(stack: &ScopeStack) -> Role {
    stack
        .as_slice()
        .iter()
        .rev()
        .find_map(scope_role)
        .unwrap_or_default()
}

fn scope_role(scope: &Scope) -> Option<Role> {
    let name = scope.build_string();
    let is = |prefix: &str| {
        name == prefix
            || name
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('.'))
    };
    Some(if is("comment") {
        Role::Comment
    } else if is("string") {
        Role::String
    } else if is("constant.numeric") || is("constant.language") || is("constant.character") {
        Role::Number
    } else if is("markup.inserted") {
        Role::Added
    } else if is("markup.deleted") {
        Role::Removed
    } else if is("entity.name.function") || is("support.function") || is("variable.function") {
        Role::Function
    } else if is("entity.name.type")
        || is("entity.name.class")
        || is("entity.name.struct")
        || is("entity.name.enum")
        || is("support.type")
        || is("support.class")
    {
        Role::Type
    } else if is("keyword") || is("storage") {
        Role::Keyword
    } else {
        return None;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles(lines: &[Vec<Token>]) -> Vec<Vec<(String, Role)>> {
        lines
            .iter()
            .map(|line| line.iter().map(|t| (t.text.clone(), t.role)).collect())
            .collect()
    }

    fn has(line: &[(String, Role)], text: &str, role: Role) -> bool {
        line.iter().any(|(t, r)| t == text && *r == role)
    }

    #[test]
    fn highlights_rust() {
        let lines = highlight("fn main() {\n    // hi\n    let n = 42;\n}", Some("rust"));
        let shown = roles(&lines);
        assert_eq!(shown.len(), 4);
        assert!(has(&shown[0], "fn", Role::Keyword), "{shown:?}");
        assert!(has(&shown[0], "main", Role::Function), "{shown:?}");
        assert!(has(&shown[1], "// hi", Role::Comment), "{shown:?}");
        assert!(has(&shown[2], "42", Role::Number), "{shown:?}");
        // Every line's text, whole.
        let text: Vec<String> = lines
            .iter()
            .map(|line| line.iter().map(|t| t.text.as_str()).collect())
            .collect();
        assert_eq!(text, ["fn main() {", "    // hi", "    let n = 42;", "}"]);
    }

    #[test]
    fn highlights_diffs_and_typescript_as_javascript() {
        let diff = roles(&highlight("-old\n+new", Some("diff")));
        assert_eq!(
            diff,
            [
                vec![("-old".to_owned(), Role::Removed)],
                vec![("+new".to_owned(), Role::Added)]
            ]
        );
        let ts = roles(&highlight("const s = \"x\";", Some("ts")));
        assert!(has(&ts[0], "\"x\"", Role::String), "{ts:?}");
    }

    #[test]
    fn names_the_theme_role_that_colours_each() {
        assert_eq!(Role::Keyword.theme_role(), "syntax-keyword");
        assert_eq!(Role::Plain.theme_role(), "body");
    }

    #[test]
    fn leaves_unknown_languages_plain() {
        assert_eq!(
            roles(&highlight("a\nb", Some("klingon"))),
            [
                vec![("a".to_owned(), Role::Plain)],
                vec![("b".to_owned(), Role::Plain)]
            ]
        );
        assert_eq!(
            roles(&highlight("a", None)),
            [vec![("a".to_owned(), Role::Plain)]]
        );
    }
}
