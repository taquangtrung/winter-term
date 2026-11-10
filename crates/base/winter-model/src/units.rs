//! How a quantity reads on screen: the units a count is shown in, where the
//! number itself would be too long to take in at a glance.

// ========================================================================
// Constants
// ========================================================================

/// The units a byte count steps through, and what each step is worth.
const SIZE_UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
const SIZE_STEP: u64 = 1024;

// ========================================================================
// Functions
// ========================================================================

/// A byte count in the largest unit that leaves it under four digits.
pub fn format_size(len: u64) -> String {
    let mut size = len as f64;
    let mut unit = 0;
    while size >= SIZE_STEP as f64 && unit + 1 < SIZE_UNITS.len() {
        size /= SIZE_STEP as f64;
        unit += 1;
    }
    if unit == 0 {
        return format!("{len}{}", SIZE_UNITS[0]);
    }
    format!("{size:.1}{}", SIZE_UNITS[unit])
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_size_steps_up_a_unit_rather_than_growing_a_digit() {
        assert_eq!(format_size(0), "0B");
        assert_eq!(format_size(1023), "1023B");
        assert_eq!(format_size(1024), "1.0K");
        assert_eq!(format_size(1024 * 1024 * 3), "3.0M");
        // Past the last unit it keeps counting in it rather than inventing one.
        assert_eq!(format_size(1024u64.pow(5)), "1024.0T");
    }
}
