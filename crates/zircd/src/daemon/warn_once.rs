//! Warnings said once: a repository that can't be scanned fails the same way at every scan.
//! The first time is a warning, and the rest are at debug until the cause changes or it's
//! put right.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The warnings said, by their path, with the cause each said.
#[derive(Debug, Default)]
pub(super) struct WarnOnce(BTreeMap<PathBuf, String>);

impl WarnOnce {
    /// Says `message` about `path`, failing because of `cause`, as a warning the first time
    /// and again whenever the cause changes; otherwise at debug. Answers whether it warned.
    pub(super) fn warn(&mut self, path: &Path, cause: &str, message: &str) -> bool {
        let key = path.to_owned();
        if self.0.get(&key).is_some_and(|said| said == cause) {
            tracing::debug!("{message} (again)");
            return false;
        }
        tracing::warn!("{message}");
        self.0.insert(key, cause.to_owned());
        true
    }

    /// `path` is fine now, or no longer watched: if it fails again, that's said again.
    pub(super) fn ok(&mut self, path: &Path) {
        self.0.remove(path);
    }

    /// Whether something about `path` has been warned of and not put right since.
    pub(super) fn said(&self, path: &Path) -> bool {
        self.0.contains_key(path)
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
        assert!(warned.warn(a, "gone", "scanning a failed"));
        assert!(!warned.warn(a, "gone", "scanning a failed"));
        assert!(warned.warn(b, "gone", "scanning b failed"), "another path");
        assert!(
            warned.warn(a, "locked", "scanning a failed"),
            "another cause"
        );
        assert!(!warned.warn(a, "locked", "scanning a failed"));
    }

    #[test]
    fn says_it_again_once_it_was_put_right_or_forgotten() {
        let mut warned = WarnOnce::default();
        let a = Path::new("/src/a");
        assert!(warned.warn(a, "gone", "scanning a failed"));
        warned.ok(a);
        assert!(warned.warn(a, "gone", "scanning a failed"));
        warned.ok(a);
        assert!(warned.warn(a, "gone", "scanning a failed"));
    }
}
