//! Links into a repository's forge: a commit or a branch named in a row becomes a link to
//! its page, from the repository's web address that commits, pushes and CI's events carry
//! (`repo`, such as `https://github.com/zircapp/monorepo`).

use super::rich::{Block, Span, SpanKind};

/// A commit's page: `<repo>/commit/<sha>`, `/-/commit/` on GitLab.
#[must_use]
pub fn commit_link(repo: &str, sha: &str) -> String {
    format!(
        "{}/{}commit/{sha}",
        repo.trim_end_matches('/'),
        gitlab(repo)
    )
}

/// A branch's page: `<repo>/tree/<branch>`, `/-/tree/` on GitLab.
#[must_use]
pub fn branch_link(repo: &str, branch: &str) -> String {
    format!(
        "{}/{}tree/{branch}",
        repo.trim_end_matches('/'),
        gitlab(repo)
    )
}

/// GitLab's pages sit under `/-/`.
fn gitlab(repo: &str) -> &'static str {
    let host = repo
        .split_once("://")
        .map_or(repo, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or_default();
    if host == "gitlab.com" || host.starts_with("gitlab.") {
        "-/"
    } else {
        ""
    }
}

/// `spans` with each whole word that `links` names, as `(word, address)`, a link to its
/// address, and inline code that is all one such word, as CI writes a target. Links and
/// mentions stay as they are, as does other code.
#[must_use]
pub fn link_words(spans: Vec<Span>, links: &[(String, String)]) -> Vec<Span> {
    if links.is_empty() {
        return spans;
    }
    let mut out = Vec::new();
    for span in spans {
        if span.kind == SpanKind::Code
            && let Some((_, url)) = links.iter().find(|(word, _)| *word == span.text)
        {
            out.push(Span {
                kind: SpanKind::Link(url.clone()),
                ..span
            });
            continue;
        }
        if span.kind != SpanKind::Text {
            out.push(span);
            continue;
        }
        let mut rest = span.text.as_str();
        while let Some((at, word, url)) = first_word(rest, links) {
            if at > 0 {
                out.push(Span {
                    text: rest[..at].to_owned(),
                    ..span.clone()
                });
            }
            out.push(Span {
                text: word.to_owned(),
                kind: SpanKind::Link(url.to_owned()),
                ..span.clone()
            });
            rest = &rest[at + word.len()..];
        }
        if !rest.is_empty() {
            out.push(Span {
                text: rest.to_owned(),
                ..span
            });
        }
    }
    out
}

/// [`link_words`] over every run of words in `blocks`: paragraphs', headings' and table
/// cells', quotes' and lists' included.
#[must_use]
pub fn link_blocks(blocks: Vec<Block>, links: &[(String, String)]) -> Vec<Block> {
    if links.is_empty() {
        return blocks;
    }
    super::rich::map_spans(blocks, &mut |spans| link_words(spans, links))
}

/// Where in `text` the first of `links`' words stands whole, which word and its address.
fn first_word<'a>(text: &str, links: &'a [(String, String)]) -> Option<(usize, &'a str, &'a str)> {
    let whole = |at: usize, len: usize| {
        let before = text[..at].chars().next_back();
        let after = text[at + len..].chars().next();
        let part = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || "-_/.".contains(c));
        // A sentence's full stop after it still leaves it whole.
        let after_ok =
            !part(after) || (after == Some('.') && !part(text[at + len + 1..].chars().next()));
        !part(before) && after_ok
    };
    links
        .iter()
        .filter(|(word, _)| !word.is_empty())
        .filter_map(|(word, url)| {
            text.match_indices(word.as_str())
                .find(|(at, _)| whole(*at, word.len()))
                .map(|(at, _)| (at, word.as_str(), url.as_str()))
        })
        .min_by_key(|(at, word, _)| (*at, std::cmp::Reverse(word.len())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(text: &str) -> Span {
        Span {
            text: text.to_owned(),
            ..Span::default()
        }
    }

    #[test]
    fn forges_pages_for_a_commit_and_a_branch() {
        assert_eq!(
            commit_link("https://github.com/zircapp/monorepo", "abc1234"),
            "https://github.com/zircapp/monorepo/commit/abc1234"
        );
        assert_eq!(
            branch_link("https://gitlab.com/acme/api/", "fix-login"),
            "https://gitlab.com/acme/api/-/tree/fix-login"
        );
    }

    #[test]
    fn links_whole_words_only() {
        let links = vec![
            ("fix".to_owned(), "https://f/fix".to_owned()),
            ("abc1234".to_owned(), "https://f/abc".to_owned()),
        ];
        let spans = link_words(vec![text("landed fix-login and fix as abc1234.")], &links);
        let shown: Vec<(&str, Option<&str>)> = spans
            .iter()
            .map(|span| {
                (
                    span.text.as_str(),
                    match &span.kind {
                        SpanKind::Link(url) => Some(url.as_str()),
                        _ => None,
                    },
                )
            })
            .collect();
        assert_eq!(
            shown,
            vec![
                ("landed fix-login and ", None),
                ("fix", Some("https://f/fix")),
                (" as ", None),
                ("abc1234", Some("https://f/abc")),
                (".", None),
            ]
        );
    }
}
