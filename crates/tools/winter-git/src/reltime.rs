//! How long ago a commit was authored, worded as compactly as a log column
//! wants it (`6h`, `3d`).
//!
//! Std-only on purpose: a date library would be a workspace dependency carried
//! for one format. The largest unit that fits is taken, truncated rather than
//! rounded, with no "ago" suffix: the column it sits in already means "ago".

// ========================================================================
// Constants
// ========================================================================

/// Seconds in each unit paired with its compact short suffix.
const SHORT_UNITS: [(i64, &str); 6] = [
    (365 * 86_400, "y"),
    (30 * 86_400, "mo"),
    (86_400, "d"),
    (3_600, "h"),
    (60, "m"),
    (1, "s"),
];

// ========================================================================
// Functions
// ========================================================================

/// How long before `now` the instant `then` was, both in seconds since the Unix
/// epoch, in compact short form such as `"6h"`, `"1m"`, `"1d"`, or `"0s"`.
pub fn short_ago(then: i64, now: i64) -> String {
    let elapsed = now.saturating_sub(then).max(0);
    for (seconds, suffix) in SHORT_UNITS {
        let count = elapsed / seconds;
        if count >= 1 {
            return format!("{count}{suffix}");
        }
    }
    format!("0{}", SHORT_UNITS[SHORT_UNITS.len() - 1].1)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// An arbitrary "now" far enough from the epoch that every unit can be
    /// subtracted from it without clamping.
    const NOW: i64 = 1_800_000_000;

    #[test]
    fn test_short_ago_formats_compact_units() {
        assert_eq!(short_ago(NOW - 5, NOW), "5s");
        assert_eq!(short_ago(NOW - 60, NOW), "1m");
        assert_eq!(short_ago(NOW - 120, NOW), "2m");
        assert_eq!(short_ago(NOW - 3_600, NOW), "1h");
        assert_eq!(short_ago(NOW - 6 * 3_600, NOW), "6h");
        assert_eq!(short_ago(NOW - 86_400, NOW), "1d");
        assert_eq!(short_ago(NOW - 3 * 86_400, NOW), "3d");
        assert_eq!(short_ago(NOW - 30 * 86_400, NOW), "1mo");
        assert_eq!(short_ago(NOW - 60 * 86_400, NOW), "2mo");
        assert_eq!(short_ago(NOW - 365 * 86_400, NOW), "1y");
        assert_eq!(short_ago(NOW - 2 * 365 * 86_400, NOW), "2y");
        assert_eq!(short_ago(NOW, NOW), "0s", "the same instant has no age");
        assert_eq!(
            short_ago(NOW + 500, NOW),
            "0s",
            "a clock-skewed stamp is no negative age"
        );
        assert_eq!(
            short_ago(0, 0),
            "0s",
            "a log line with no timestamp does not read as 1970"
        );
    }
}
