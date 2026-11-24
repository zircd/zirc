//! The server's own nicks, each connected to Ergo's loopback as an account of zirc's own: a
//! role's (`crate::roles`). What keeps one connected and passes on what it hears is
//! [`connection`].

use std::time::Duration;

/// How long a nick's connection waits before connecting again, at first and at most.
pub(crate) const RETRY: Duration = Duration::from_secs(5);
pub(crate) const RETRY_MOST: Duration = Duration::from_mins(5);

/// How long connecting may take.
pub(crate) const WITHIN: Duration = Duration::from_secs(8);

pub mod connection;

/// Whether `account` is zirc's own, not a person's or a machine's: the recorder, or a role's
/// nick. Each is always connected and counts as nobody (`crate::accounts::sessions`).
#[must_use]
pub fn is_zircs_own(account: &str) -> bool {
    let account = account.to_lowercase();
    account == zirc_proto::activity::RECORDER || crate::roles::is_role(&account)
}
