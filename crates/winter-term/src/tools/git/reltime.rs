//! How long ago a commit was authored, worded the way a log view wants it.
//!
//! Std-only on purpose: the git views are the only thing that needs a duration
//! in words, and a date library would be a workspace dependency carried for one
//! format string. The thresholds match the strict distance formatting a reader
//! expects — the largest unit that fits, truncated rather than rounded, with no
//! "ago" suffix, because the column it sits in already means "ago".

// ========================================================================
// Constants
// ========================================================================

/// Seconds in each unit, largest first, paired with the unit's singular name.
/// A month is 30 days and a year 365: a log column is not the place to care
/// that months differ in length.
const UNITS: [(i64, &str); 6] = [
    (365 * 86_400, "year"),
    (30 * 86_400, "month"),
    (86_400, "day"),
    (3_600, "hour"),
    (60, "minute"),
    (1, "second"),
];

// ========================================================================
// Functions
// ========================================================================

/// How long before `now` the instant `then` was, both in seconds since the Unix
/// epoch, as `"2 hours"` or `"1 day"`.
///
/// A `then` in the future, or a `then` of `0` meaning the log carried no
/// timestamp, both read as `"0 seconds"`: a log row shows an age or it shows
/// nothing useful, and neither case is worth a second wording.
pub fn ago(then: i64, now: i64) -> String {
    let elapsed = now.saturating_sub(then).max(0);
    for (seconds, name) in UNITS {
        let count = elapsed / seconds;
        if count >= 1 {
            return plural(count, name);
        }
    }
    plural(0, "second")
}

/// `count` of `unit`, pluralized the way English does it.
fn plural(count: i64, unit: &str) -> String {
    if count == 1 {
        format!("{count} {unit}")
    } else {
        format!("{count} {unit}s")
    }
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
    fn test_each_unit_is_named_at_its_own_scale() {
        assert_eq!(ago(NOW - 5, NOW), "5 seconds");
        assert_eq!(ago(NOW - 120, NOW), "2 minutes");
        assert_eq!(ago(NOW - 7_200, NOW), "2 hours");
        assert_eq!(ago(NOW - 3 * 86_400, NOW), "3 days");
        assert_eq!(ago(NOW - 60 * 86_400, NOW), "2 months");
        assert_eq!(ago(NOW - 2 * 365 * 86_400, NOW), "2 years");
    }

    #[test]
    fn test_a_single_unit_reads_singular() {
        assert_eq!(ago(NOW - 1, NOW), "1 second");
        assert_eq!(ago(NOW - 60, NOW), "1 minute");
        assert_eq!(ago(NOW - 3_600, NOW), "1 hour");
        assert_eq!(ago(NOW - 86_400, NOW), "1 day");
        assert_eq!(ago(NOW - 30 * 86_400, NOW), "1 month");
        assert_eq!(ago(NOW - 365 * 86_400, NOW), "1 year");
    }

    #[test]
    fn test_the_largest_fitting_unit_wins_and_truncates() {
        // 90 minutes is an hour and a half, which reads as the hour it passed.
        assert_eq!(ago(NOW - 5_400, NOW), "1 hour");
        // One second short of the next unit still reads as the smaller one.
        assert_eq!(ago(NOW - 59, NOW), "59 seconds");
        assert_eq!(ago(NOW - 86_399, NOW), "23 hours");
    }

    #[test]
    fn test_no_age_and_future_stamps_read_as_zero() {
        assert_eq!(ago(NOW, NOW), "0 seconds", "the same instant has no age");
        assert_eq!(
            ago(NOW + 500, NOW),
            "0 seconds",
            "a clock-skewed future stamp does not read as a negative age"
        );
        assert_eq!(
            ago(0, 0),
            "0 seconds",
            "a log line with no timestamp does not read as 1970"
        );
    }
}
