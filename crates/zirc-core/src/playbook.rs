//! A team's playbook: what its agents hear at the right moment, in one place. That is its
//! hints ([`crate::hints`]), what to do when something happens, from the person's config and
//! the repository's `.zirc/hints.toml` over zirc's defaults; and its gotchas
//! ([`crate::gotchas`]), lessons tied to paths. `zirc playbook <path>` shows those that apply
//! there: hints aren't tied to paths, so a repository's apply throughout it.

use serde::Serialize;

use crate::gotchas::{self, Gotcha};
use crate::hints::{self, Hint, Layer};

/// An id for a lesson or a hint, from its words: the first 8 bytes of their SHA-256, in hex.
/// What usage records and retirings name, so no words leave the room they're kept in.
#[must_use]
pub fn fingerprint(text: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, text.as_bytes());
    zirc_proto::hex(&digest.as_ref()[..8])
}

/// One hint and its layer, as `--json` gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry<'a> {
    /// Where it comes from.
    pub layer: Layer,
    /// The hint, as its file says it.
    #[serde(flatten)]
    pub hint: &'a Hint,
}

/// The playbook for a path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Playbook<'a> {
    /// Every hint, highest layer first, zirc's defaults included.
    pub hints: Vec<Entry<'a>>,
    /// The gotchas about the path, what's in it or what holds it, newest first.
    pub gotchas: Vec<&'a Gotcha>,
}

impl<'a> Playbook<'a> {
    /// The playbook from `layers`, each with its hints, highest first, and `gotchas` already
    /// narrowed to the path ([`gotchas::under`]).
    #[must_use]
    pub fn new(layers: &[(Layer, &'a [Hint])], gotchas: Vec<&'a Gotcha>) -> Self {
        let hints = layers
            .iter()
            .flat_map(|(layer, hints)| {
                hints.iter().map(|hint| Entry {
                    layer: *layer,
                    hint,
                })
            })
            .collect();
        Self { hints, gotchas }
    }

    /// As `zirc playbook` prints it: the team's own hints in full, zirc's defaults as a
    /// count, then every gotcha about `place`, such as `web/e2e` or `the repository`.
    #[must_use]
    pub fn text(&self, place: &str) -> String {
        use std::fmt::Write as _;
        let (defaults, own): (Vec<&Entry>, Vec<&Entry>) = self
            .hints
            .iter()
            .partition(|entry| entry.layer == Layer::Default);
        let mut text = String::from("hints, what agents do when something happens:\n");
        if own.is_empty() {
            text.push_str(
                "  none of your own yet: the repository's .zirc/hints.toml or your \
                 ~/.zirc/config.toml adds them\n",
            );
        }
        for entry in &own {
            let _ = writeln!(text, "  {}", hints::line(entry.layer, entry.hint));
        }
        if !defaults.is_empty() {
            let _ = writeln!(
                text,
                "  and {} of zirc's own, for what yours don't cover: `zirc hints`",
                defaults.len()
            );
        }
        let _ = writeln!(text, "gotchas about {place}, newest first:");
        if self.gotchas.is_empty() {
            text.push_str("  none yet: `zirc gotcha <paths> -m \"<lesson>\"` keeps one\n");
        }
        for gotcha in &self.gotchas {
            let _ = writeln!(text, "  {}", gotchas::line(gotcha));
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hints::{Level, When};

    fn hint(kind: &str, level: Level, text: &str) -> Hint {
        Hint {
            kind: kind.into(),
            when: When::default(),
            level,
            text: text.into(),
            required: false,
        }
    }

    fn gotcha(paths: &[&str], text: &str, at: &str) -> Gotcha {
        Gotcha {
            paths: paths.iter().map(|&path| path.to_owned()).collect(),
            text: text.into(),
            who: "cc-1".into(),
            at: Some(at.into()),
            id: None,
        }
    }

    #[test]
    fn shows_the_teams_hints_counts_zircs_and_lists_the_gotchas() {
        let repo = [Hint {
            when: When {
                lockfile: Some(true),
                ..When::default()
            },
            required: true,
            ..hint("overlap", Level::Act, "regenerate it after")
        }];
        let personal = [hint("landed", Level::Quiet, "nothing to do")];
        let defaults = hints::defaults();
        let all = [
            gotcha(&["web/e2e"], "specs share state", "2026-11-12T10:00:00Z"),
            gotcha(&["web/src"], "pnpm only", "2026-11-13T10:00:00Z"),
        ];
        let layers = [
            (Layer::Personal, &personal[..]),
            (Layer::Repository, &repo[..]),
            (Layer::Default, defaults.as_slice()),
        ];
        let playbook = Playbook::new(&layers, gotchas::under(&all, &["web/e2e".to_owned()]));
        let text = playbook.text("web/e2e");
        assert_eq!(
            text,
            format!(
                "hints, what agents do when something happens:\n\
                 \x20 landed (quiet, personal): nothing to do\n\
                 \x20 overlap (act, repo, required) when lockfile = true: regenerate it after\n\
                 \x20 and {} of zirc's own, for what yours don't cover: `zirc hints`\n\
                 gotchas about web/e2e, newest first:\n\
                 \x20 2026-11-12 cc-1: specs share state (web/e2e)\n",
                defaults.len()
            )
        );
        // JSON has every hint with its layer, the hint's own fields beside it.
        let json = serde_json::to_value(&playbook).unwrap();
        assert_eq!(json["hints"][1]["layer"], "repository");
        assert_eq!(json["hints"][1]["when"]["lockfile"], true);
        assert_eq!(json["hints"].as_array().unwrap().len(), 2 + defaults.len());
        assert_eq!(json["gotchas"][0]["text"], "specs share state");
    }

    #[test]
    fn a_fingerprint_is_short_and_says_which_words() {
        let one = fingerprint("pnpm only");
        assert_eq!(one.len(), 16);
        assert!(one.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(one, fingerprint("pnpm only"));
        assert_ne!(one, fingerprint("pnpm only."));
    }

    #[test]
    fn says_how_to_start_one_when_a_team_has_none() {
        let playbook = Playbook::new(&[], Vec::new());
        let text = playbook.text("the repository");
        assert!(text.contains("none of your own yet"), "{text}");
        assert!(text.contains("gotchas about the repository"), "{text}");
        assert!(text.contains("none yet: `zirc gotcha"), "{text}");
        assert!(!text.contains("of zirc's own"), "{text}");
    }
}
