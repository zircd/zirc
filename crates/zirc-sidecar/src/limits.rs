//! The limits on a server's desired state and on what its sidecar keeps, in one place: the
//! controller's `ZircServer` schema holds the API server to them, the controller and the sidecar
//! hold to them, and zirc.dev reads them from the TypeScript the controller generates, so none
//! is declared twice.

// The bounds on what zirc.dev wants of a server (teams, owners, settings, public channels and
// token keys) are the desired state's own, in zirc-proto.
pub use zirc_proto::desired::limits::*;

/// The organisation's JWT key (PEM), as the spec's `webJwtKey` had it before `tokenKeys`.
pub const WEB_JWT_KEY_CHARS: usize = TOKEN_KEY_CHARS;
/// The organisation's JWT key's pattern, as the spec's `webJwtKey` had it before `tokenKeys`.
pub const WEB_JWT_KEY_PATTERN: &str = TOKEN_KEY_PATTERN;

/// Guests: at once on the server, and accounts an organisation may have.
pub const GUESTS_AT_ONCE: usize = 50;
/// The most guest accounts an organisation may have.
pub const GUEST_ACCOUNTS_MAX: usize = 1000;

/// A profile's name and picture URL (`crate::profiles`).
pub const PROFILE_NAME_CHARS: usize = 100;
/// The longest avatar URL.
pub const AVATAR_CHARS: usize = 2048;

/// The size of a tenant's disk, in GiB, which the sidecar reports as its history's room.
pub const DISK_GIB: u32 = 10;
