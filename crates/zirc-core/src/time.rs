//! IRC's `time` tags (`2026-11-02T11:25:07.123Z`), the server's clock on every message, to
//! and from seconds since the Unix epoch.

/// The time `tag` says, in seconds since the Unix epoch, if it's one.
#[must_use]
pub fn seconds_at(tag: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(tag)
        .ok()
        .and_then(|at| u64::try_from(at.timestamp()).ok())
}

/// The time `tag` says, in milliseconds since the Unix epoch, if it's one: for telling
/// apart messages the server got within a second of each other.
#[must_use]
pub fn millis_at(tag: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(tag)
        .ok()
        .and_then(|at| u64::try_from(at.timestamp_millis()).ok())
}

/// `seconds` since the Unix epoch as a `time` tag.
#[must_use]
pub fn tag(seconds: u64) -> String {
    i64::try_from(seconds)
        .ok()
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// The moment `text` names, in seconds since the Unix epoch, given the time `now`: a time as
/// RFC 3339 has it (`2026-10-31T08:00:00Z`), a day (`2026-10-31`, its midnight UTC), or how
/// long ago (`90s`, `30m`, `36h`, `6d`). `None` for anything else.
#[must_use]
pub fn moment(text: &str, now: u64) -> Option<u64> {
    if let Some(at) = seconds_at(text) {
        return Some(at);
    }
    if let Ok(day) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return u64::try_from(day.and_hms_opt(0, 0, 0)?.and_utc().timestamp()).ok();
    }
    now.checked_sub(duration(text)?)
}

/// How long `text` says, in seconds, as zirc writes a length of time everywhere
/// ([`zirc_proto::duration`]).
pub use zirc_proto::duration;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_time_tag_to_the_millisecond() {
        assert_eq!(
            millis_at("2026-11-02T11:25:07.123Z"),
            Some(1_793_618_707_123)
        );
        assert_eq!(millis_at("yesterday"), None);
    }

    #[test]
    fn reads_moments_as_times_days_or_how_long_ago() {
        let now = 1_793_618_707;
        assert_eq!(moment("2026-11-02T11:25:07.123Z", now), Some(1_793_618_707));
        assert_eq!(moment("2026-10-31", now), Some(1_793_404_800));
        assert_eq!(moment("90s", now), Some(now - 90));
        assert_eq!(moment("30m", now), Some(now - 1800));
        assert_eq!(moment("36h", now), Some(now - 36 * 3600));
        assert_eq!(moment("6d", now), Some(now - 6 * 86_400));
        assert_eq!(moment("2w", now), Some(now - 14 * 86_400));
        for bad in ["", "h", "30", "30y", "-3h", "yesterday"] {
            assert_eq!(moment(bad, now), None, "{bad}");
        }
        assert_eq!(duration("90s"), Some(90));
        assert_eq!(duration("2w"), Some(1_209_600));
        assert_eq!(
            duration("20"),
            None,
            "a unit, always: a flag says what a bare number is"
        );
    }

    #[test]
    fn reads_and_writes_time_tags() {
        assert_eq!(seconds_at("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(seconds_at("2026-11-02T11:25:07.123Z"), Some(1_793_618_707));
        assert_eq!(seconds_at("2024-02-29T23:59:59Z"), Some(1_709_251_199));
        assert_eq!(seconds_at("yesterday"), None);
        assert_eq!(tag(1_793_618_707), "2026-11-02T11:25:07.000Z");
        for seconds in [0, 1_709_251_199, 951_782_400] {
            assert_eq!(seconds_at(&tag(seconds)), Some(seconds));
        }
    }
}
