//! The one clock misty and Cherry share: misty's `Instant`s as Cherry
//! [`Timestamp`]s, measured from the first moment anything read the clock.
//! [`EditorView`](crate::EditorView) reads it; an app driving a Cherry
//! `Resolver` or `Editor` itself passes [`now`] too, and turns a deadline
//! Cherry gives back into an `Instant` with [`instant`], so every editor and
//! keymap in the process sees one timeline for its key timeouts and double
//! clicks.

use std::sync::OnceLock;
use std::time::Instant;

use cherry_keys::Timestamp;

// ========================================================================
// Constants
// ========================================================================

/// The moment the clock started, the origin of every timestamp.
static ORIGIN: OnceLock<Instant> = OnceLock::new();

// ========================================================================
// Functions
// ========================================================================

/// Now, as Cherry takes it.
pub fn now() -> Timestamp {
    stamp(Instant::now())
}

/// `at` as Cherry takes it; a moment before the clock started is its
/// start.
pub fn stamp(at: Instant) -> Timestamp {
    Timestamp::from_duration(at.saturating_duration_since(origin()))
}

/// The moment `at`, a timestamp of this clock, falls on: a Cherry
/// deadline as the `Instant` misty's `Application::about_to_wait` returns.
pub fn instant(at: Timestamp) -> Instant {
    origin() + at.as_duration()
}

/// The moment the clock started: the first time any of it was read.
fn origin() -> Instant {
    *ORIGIN.get_or_init(Instant::now)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn test_a_moment_comes_back_from_its_timestamp_and_an_earlier_one_is_the_start() {
        let later = Instant::now() + Duration::from_millis(250);
        assert_eq!(instant(stamp(later)), later);
        assert_eq!(
            stamp(origin() - Duration::from_secs(1)),
            Timestamp::from_millis(0)
        );
    }
}
