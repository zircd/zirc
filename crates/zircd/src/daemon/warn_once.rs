//! Warnings said once: a repository that can't be scanned, or whose `.zirc/config.toml` this
//! zircd can't read, fails the same way at every scan and post. The first time is a warning,
//! and the rest are at debug until the cause changes or it's put right.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What a warning is about, besides its path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum About {
    /// Reading a repository with Git.
    Scan,
    /// A repository's team config.
    TeamConfig,
}

/// The warnings said, by what they're about and their path, with the cause each said.
#[derive(Debug, Default)]
pub(super) struct WarnOnce(BTreeMap<(About, PathBuf), String>);

impl WarnOnce {
    /// Says `message` about `path`, failing because of `cause`, as a warning the first time
    /// and again whenever the cause changes; otherwise at debug. Answers whether it warned.
    pub(super) fn warn(&mut self, about: About, path: &Path, cause: &str, message: &str) -> bool {
        let key = (about, path.to_owned());
        if self.0.get(&key).is_some_and(|said| said == cause) {
            tracing::debug!("{message} (again)");
            return false;
        }
        tracing::warn!("{message}");
        self.0.insert(key, cause.to_owned());
        true
    }

    /// `path` is fine now as far as `about` goes: if it fails again, that's said again.
    pub(super) fn ok(&mut self, about: About, path: &Path) {
        self.0.remove(&(about, path.to_owned()));
    }

    /// Whether something about `path` has been warned of and not put right since.
    pub(super) fn said(&self, about: About, path: &Path) -> bool {
        self.0.contains_key(&(about, path.to_owned()))
    }

    /// `path` is no longer watched: nothing about it is kept.
    pub(super) fn forget(&mut self, path: &Path) {
        self.0.retain(|(_, said), _| said != path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_a_warning_once_per_path_and_cause() {
        let mut warned = WarnOnce::default();
        let a = Path::new("/src/a");
        let b = Path::new("/src/b");
        assert!(warned.warn(About::Scan, a, "gone", "scanning a failed"));
        assert!(!warned.warn(About::Scan, a, "gone", "scanning a failed"));
        assert!(
            warned.warn(About::Scan, b, "gone", "scanning b failed"),
            "another path"
        );
        assert!(
            warned.warn(About::TeamConfig, a, "gone", "a's config"),
            "another kind"
        );
        assert!(
            warned.warn(About::Scan, a, "locked", "scanning a failed"),
            "another cause"
        );
        assert!(!warned.warn(About::Scan, a, "locked", "scanning a failed"));
    }

    #[test]
    fn says_it_again_once_it_was_put_right_or_forgotten() {
        let mut warned = WarnOnce::default();
        let a = Path::new("/src/a");
        assert!(warned.warn(About::Scan, a, "gone", "scanning a failed"));
        warned.ok(About::Scan, a);
        assert!(warned.warn(About::Scan, a, "gone", "scanning a failed"));
        assert!(warned.warn(About::TeamConfig, a, "bad", "a's config"));
        warned.forget(a);
        assert!(warned.warn(About::Scan, a, "gone", "scanning a failed"));
        assert!(warned.warn(About::TeamConfig, a, "bad", "a's config"));
    }
}
