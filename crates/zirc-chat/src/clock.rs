//! What time it is, and on what clock it's told: the one place zirc's apps read either, so
//! the store screenshots can stop both. Normally the system's time and this machine's time
//! zone; once [`pin`]ned, for the rest of the process, the moment and offset pinned, which
//! only the showcase does (`zirc-app`'s `showcase`, played by the screenshot hosts), so its
//! shots look the same whenever and wherever they're made.
//!
//! Not for the browser, where the system's time panics: the web client hands in its own
//! ([`crate::chat::Chat::clock`]).

use std::sync::OnceLock;

use chrono::{DateTime, FixedOffset, Local, NaiveDate, Utc};

/// The moment, and the offset its clock reads, once pinned.
static PINNED: OnceLock<DateTime<FixedOffset>> = OnceLock::new();

/// Stops the clock at `at`, told on `at`'s offset, for the rest of the process: for the
/// showcase alone. The first pin holds; a later one changes nothing.
pub fn pin(at: DateTime<FixedOffset>) {
    let _ = PINNED.set(at);
}

/// Now: the system's time, or the pinned moment.
#[must_use]
pub fn now() -> DateTime<Utc> {
    PINNED.get().map_or_else(Utc::now, DateTime::to_utc)
}

/// `at` on this machine's clock, in its time zone then, or on the pinned offset.
#[must_use]
pub fn local(at: DateTime<Utc>) -> DateTime<FixedOffset> {
    match PINNED.get() {
        Some(pinned) => at.with_timezone(pinned.offset()),
        None => at.with_timezone(&Local).fixed_offset(),
    }
}

/// This machine's clock's offset from UTC now, or the pinned offset.
#[must_use]
pub fn offset() -> FixedOffset {
    *local(now()).offset()
}

/// Today, on this machine's clock, or the pinned moment's day.
#[must_use]
pub fn today() -> NaiveDate {
    local(now()).date_naive()
}
