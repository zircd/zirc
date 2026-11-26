//! The clock pinned, as the showcase pins it: a test of its own, as the pin holds for the
//! whole process.

use chrono::{DateTime, FixedOffset, NaiveDate};
use zirc_chat::chat::{Chat, Options};
use zirc_chat::clock;

#[test]
fn a_pin_stops_the_clock_on_its_offset() {
    let at = DateTime::parse_from_rfc3339("2026-10-15T09:41:00+00:00").unwrap();
    clock::pin(at);
    // A later pin changes nothing.
    clock::pin(DateTime::parse_from_rfc3339("2030-01-01T00:00:00+05:00").unwrap());
    assert_eq!(clock::now(), at);
    assert_eq!(clock::offset(), FixedOffset::east_opt(0).unwrap());
    assert_eq!(
        clock::today(),
        NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
    );
    let late = DateTime::parse_from_rfc3339("2026-10-15T23:30:00-02:00").unwrap();
    assert_eq!(
        clock::local(late.to_utc()).date_naive(),
        NaiveDate::from_ymd_opt(2026, 10, 16).unwrap()
    );
    // A chat reads it too.
    let chat = Chat::new("henrik", "irc.zirc.test", Vec::new(), Options::default());
    assert_eq!((chat.clock)(), at);
}
