//! Words about the person's office that several places share, decided once: the apps' Office
//! when it has no coworkers yet and a coworker's question waiting, the header's way into the
//! office on zirc.dev, and the hire step's. zirc.dev's home page itself is the same for everyone,
//! signed in or not (the user's choice, 28 Nov 2026): it once drew the person's office, or where
//! they left off setting it up, instead of its hero.

/// The way to hire with no coworkers yet, as the apps' Office says it under [`NO_COWORKERS`].
pub const HIRE_FIRST: &str = "Hire a coworker";

/// In place of the badges with no coworkers yet, as the apps' Office says it.
pub const NO_COWORKERS: &str = "No coworkers yet.";

/// A coworker's badge strip while one of its questions waits for the person, as the apps'
/// Office says it.
pub const ASKED_YOU: &str = "Asked you";

/// The header's button, signed in, into the person's office.
pub const YOUR_OFFICE: &str = "Your office";

/// A coworker hired in a browser before there's an office, as sign-up shows it.
pub const KEPT_STRIP: &str = "Hired, no office yet";

/// The hire step's heading.
pub const HIRE_YOUR_FIRST: &str = "Hire your first coworker";

/// The TypeScript of what zirc.dev reads.
#[must_use]
pub fn typescript() -> Vec<String> {
    vec![
        format!("const HOME_ASKED_YOU = {ASKED_YOU:?};"),
        format!("const HOME_HIRE_FIRST = {HIRE_FIRST:?};"),
        format!("const HOME_YOUR_OFFICE = {YOUR_OFFICE:?};"),
        format!("const HOME_KEPT_STRIP = {KEPT_STRIP:?};"),
        format!("const HOME_HIRE_YOUR_FIRST = {HIRE_YOUR_FIRST:?};"),
    ]
}
