//! Which version of zirc a program is, said once as it starts ([`set`]) and read wherever a
//! library says it ([`get`]): to a server, in a request's `User-Agent`, to `zirc update`.
//!
//! A library isn't compiled with the version: a release changes it, and compiled into every
//! crate it made the build after a release compile every one of them again, in each of five
//! configurations (build 10553 took 45 minutes on a cache a version bump had emptied). Only
//! what's run is: a binary's `main.rs`, and the apps' libraries. Each says it first thing,
//! and `//release:versions_test` runs the ones that ship to see they do.

use std::sync::OnceLock;

static VERSION: OnceLock<&'static str> = OnceLock::new();

/// What [`get`] says of a program that hasn't said its version, as a test is: Bazel's own
/// default for a crate's.
pub const UNSET: &str = "0.0.0";

/// Says this program's version, once: what's said after the first is ignored.
pub fn set(version: &'static str) {
    let _ = VERSION.set(version);
}

/// This program's version, `major.minor.patch`.
#[must_use]
pub fn get() -> &'static str {
    VERSION.get().copied().unwrap_or(UNSET)
}

/// What zirc's requests say they're from: `zirc/<version>`.
#[must_use]
pub fn user_agent() -> String {
    format!("{}/{}", crate::PROGRAM, get())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_says_its_version_once_and_unsaid_its_the_default() {
        // One test, as the version is the process's: unsaid first.
        assert_eq!(get(), UNSET);
        set("1.2.3");
        set("4.5.6");
        assert_eq!((get(), user_agent().as_str()), ("1.2.3", "zirc/1.2.3"));
    }
}
