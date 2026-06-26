//! When a page that keeps itself current asks for its next snapshot: shared by
//! the process and system monitors, so the timing rules live in one place.

use std::time::{Duration, Instant};

// ========================================================================
// Data Structures
// ========================================================================

/// The schedule of a page's refreshes.
#[derive(Clone, Copy, Debug)]
pub struct RefreshTimer {
    /// When the snapshot now outstanding was asked for.
    awaiting: Option<Instant>,
    /// How long after an answer the next snapshot is asked for.
    interval: Duration,
    /// When the next snapshot is due.
    next: Instant,
    paused: bool,
    /// How long a snapshot may be outstanding before it is given up on and
    /// asked for again. The runner drops a request when it is busy, so without
    /// this a dropped one would leave the page waiting forever.
    stall: Duration,
}

// ========================================================================
// RefreshTimer
// ========================================================================

impl RefreshTimer {
    /// A timer whose first snapshot is due at once.
    pub fn new(interval: Duration, stall: Duration) -> Self {
        Self {
            awaiting: None,
            interval,
            next: Instant::now(),
            paused: false,
            stall,
        }
    }

    /// When the page next wants a tick: the give-up deadline while a snapshot
    /// is out, the due time otherwise, and never while paused.
    pub fn next_tick(&self) -> Option<Instant> {
        match self.awaiting {
            Some(since) => Some(since + self.stall),
            None if self.paused => None,
            None => Some(self.next),
        }
    }

    /// The deadline passed. Whether a snapshot should now be asked for: not
    /// while paused, and any outstanding one is given up on.
    pub fn on_tick(&mut self, now: Instant) -> bool {
        if self.paused {
            self.awaiting = None;
            return false;
        }
        self.awaiting = Some(now);
        true
    }

    /// A snapshot is being asked for outside the schedule.
    pub fn request_now(&mut self, now: Instant) {
        self.awaiting = Some(now);
    }

    /// The snapshot arrived, or failed: the next is one interval away.
    pub fn on_answer(&mut self, now: Instant) {
        self.awaiting = None;
        self.next = now + self.interval;
    }

    /// Something happened that makes the list wrong: do not wait for the
    /// interval.
    pub fn refresh_soon(&mut self, now: Instant) {
        self.next = now;
    }

    /// Start over, as a page does when it comes back from under another.
    pub fn restart(&mut self, now: Instant) {
        self.awaiting = None;
        self.next = now;
    }

    /// Pause or resume. Resuming refreshes at once.
    pub fn toggle_pause(&mut self, now: Instant) {
        self.paused = !self.paused;
        if !self.paused {
            self.next = now;
        }
    }

    /// Whether refreshing is paused.
    pub fn is_paused(&self) -> bool {
        self.paused
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const INTERVAL: Duration = Duration::from_secs(2);
    const STALL: Duration = Duration::from_secs(10);

    #[test]
    fn test_a_tick_asks_once_and_then_waits_for_the_give_up_deadline() {
        let mut timer = RefreshTimer::new(INTERVAL, STALL);
        let now = Instant::now();
        assert!(timer.on_tick(now));
        // With one outstanding the next wake is the give-up deadline, not
        // another request.
        assert_eq!(timer.next_tick(), Some(now + STALL));
    }

    #[test]
    fn test_an_answer_schedules_the_next_snapshot_one_interval_out() {
        let mut timer = RefreshTimer::new(INTERVAL, STALL);
        let now = Instant::now();
        timer.on_tick(now);
        timer.on_answer(now);
        assert_eq!(timer.next_tick(), Some(now + INTERVAL));
    }

    #[test]
    fn test_a_snapshot_that_never_came_back_is_asked_for_again() {
        let mut timer = RefreshTimer::new(INTERVAL, STALL);
        let now = Instant::now();
        timer.on_tick(now);
        // Retrying at the give-up deadline is what stops a dropped request
        // from leaving the page waiting forever.
        assert!(timer.on_tick(now + STALL));
    }

    #[test]
    fn test_a_paused_timer_asks_for_nothing_and_wakes_nobody_even_mid_request() {
        let mut timer = RefreshTimer::new(INTERVAL, STALL);
        let now = Instant::now();
        timer.on_tick(now);
        timer.toggle_pause(now);
        // The give-up deadline still fires, to clear the outstanding request.
        assert!(!timer.on_tick(now + STALL));
        assert_eq!(timer.next_tick(), None);
        timer.toggle_pause(now);
        assert_eq!(timer.next_tick(), Some(now));
    }
}
