//! How the monitor words a size and a percentage.

// ========================================================================
// Constants
// ========================================================================

/// Kilobytes in a megabyte, and so on up: the steps between units.
const KB_PER_UNIT: u64 = 1024;

/// Units after kilobytes, smallest first.
const LARGER_UNITS: [char; 3] = ['M', 'G', 'T'];

/// A percentage at or past this is shown without a decimal, since a tenth of a
/// percent means nothing next to three digits.
const WHOLE_PERCENT_FROM: f64 = 100.0;

// ========================================================================
// Functions
// ========================================================================

/// A size in kilobytes as the largest unit it fills: `812K`, `4.2M`, `1.5G`.
pub fn format_kb(kb: u64) -> String {
    if kb < KB_PER_UNIT {
        return format!("{kb}K");
    }
    let mut value = kb as f64 / KB_PER_UNIT as f64;
    let mut unit = LARGER_UNITS[0];
    for larger in &LARGER_UNITS[1..] {
        if value < KB_PER_UNIT as f64 {
            break;
        }
        value /= KB_PER_UNIT as f64;
        unit = *larger;
    }
    format!("{value:.1}{unit}")
}

/// A percentage to one decimal place, or none once it passes a hundred.
pub fn format_percent(percent: f64) -> String {
    if !percent.is_finite() || percent <= 0.0 {
        return "0.0".to_string();
    }
    if percent >= WHOLE_PERCENT_FROM {
        return format!("{percent:.0}");
    }
    format!("{percent:.1}")
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_size_moves_up_a_unit_at_a_thousand_and_twenty_four() {
        assert_eq!(format_kb(1023), "1023K");
        assert_eq!(format_kb(1024), "1.0M");
        assert_eq!(format_kb(1024 * 1024), "1.0G");
        assert_eq!(format_kb(1024 * 1024 * 1024), "1.0T");
    }

    #[test]
    fn test_a_size_past_the_largest_unit_stays_in_it() {
        assert_eq!(format_kb(5 * 1024 * 1024 * 1024 * 1024), "5120.0T");
    }

    #[test]
    fn test_a_nonsense_percentage_reads_as_zero() {
        assert_eq!(format_percent(f64::NAN), "0.0");
        assert_eq!(format_percent(-3.0), "0.0");
    }

    #[test]
    fn test_a_percentage_loses_its_decimal_at_a_hundred() {
        assert_eq!(format_percent(99.94), "99.9");
        assert_eq!(format_percent(250.4), "250");
    }
}
