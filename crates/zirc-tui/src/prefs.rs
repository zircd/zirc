//! What the client keeps for the person between runs, in `tui-prefs.toml` in zirc's
//! directory: the conversations they pinned, whether others see them typing, and how the
//! scrollback's laid out. The run loop saves it when [`crate::App`] says it changed.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

/// What's kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// The conversations pinned to the top of their section, in the order pinned.
    pub pins: Vec<String>,
    /// Whether others are told when we're typing.
    pub send_typing: bool,
    /// Whether the scrollback is laid out IRC's way.
    pub compact: bool,
    /// The conversations showing only what people say (`/people`).
    pub people_only: Vec<String>,
    /// The editor files named in messages open in, by its id: `vs-code`, `cursor`, `zed`
    /// or `xcode`. The first installed when unset.
    pub editor: Option<String>,
    /// What conversations show where the person picked it (`/show`), by name: `error`,
    /// `warn`, `info` or `debug`.
    pub thresholds: std::collections::BTreeMap<String, String>,
    /// The direct messages closed (`/close`), by name: when, in milliseconds since 1970.
    pub closed: std::collections::BTreeMap<String, i64>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            pins: Vec::new(),
            send_typing: true,
            compact: false,
            people_only: Vec::new(),
            editor: None,
            thresholds: std::collections::BTreeMap::new(),
            closed: std::collections::BTreeMap::new(),
        }
    }
}

/// Where it's kept, in zirc's directory `home`.
#[must_use]
pub fn path(home: &Path) -> PathBuf {
    home.join("tui-prefs.toml")
}

impl Prefs {
    /// What's kept at `path`: the defaults when there's nothing, or it can't be read, so a
    /// bad file never stops the client starting.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Keeps it at `path`.
    ///
    /// # Errors
    ///
    /// When the file can't be written.
    ///
    /// # Panics
    ///
    /// Never: every field serializes to TOML.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string(self).expect("prefs serialize to TOML");
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_what_was_saved_and_starts_from_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = path(dir.path());
        assert_eq!(Prefs::load(&path), Prefs::default());
        let prefs = Prefs {
            pins: vec!["#api".into(), "bob".into()],
            send_typing: false,
            compact: true,
            people_only: vec!["#web".into()],
            editor: Some("zed".into()),
            thresholds: [("#ops".to_owned(), "info".to_owned())].into(),
            closed: [("mattias-build-2301".to_owned(), 1_793_000_000_000)].into(),
        };
        prefs.save(&path).unwrap();
        assert_eq!(Prefs::load(&path), prefs);
        std::fs::write(&path, "pins = 3").unwrap();
        assert_eq!(
            Prefs::load(&path),
            Prefs::default(),
            "unreadable, the defaults"
        );
    }
}
