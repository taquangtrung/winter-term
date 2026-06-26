//! A two-phase blink clock: shown, then hidden, flipping every period while
//! something on screen is blinking, and holding shown otherwise.

use std::time::{Duration, Instant};

use super::App;

// ========================================================================
// Constants
// ========================================================================

/// How long blinking text (SGR 5) spends in each phase. Slower than the
/// cursor, as in xterm, so text flashing beside a blinking cursor reads as
/// a separate signal.
pub(super) const TEXT_BLINK_PERIOD: Duration = Duration::from_millis(800);

// ========================================================================
// Data Structures
// ========================================================================

/// A blink phase and when it next flips.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BlinkTimer {
    next_flip: Instant,
    period: Duration,
    visible: bool,
}

// ========================================================================
// BlinkTimer
// ========================================================================

impl BlinkTimer {
    /// A timer in its shown phase, first flipping one `period` from `now`.
    pub(crate) fn new(period: Duration, now: Instant) -> Self {
        Self {
            next_flip: now + period,
            period,
            visible: true,
        }
    }

    /// Whether the blinking thing is in its shown phase.
    pub(crate) fn is_visible(&self) -> bool {
        self.visible
    }

    /// Advance to `now`. While `active`, flip once the period is up; while
    /// not, hold the shown phase and restart the period, so blinking resumes
    /// from shown rather than mid-cycle. Returns whether the phase changed.
    pub(crate) fn tick(&mut self, now: Instant, active: bool) -> bool {
        if !active {
            let changed = !self.visible;
            self.visible = true;
            self.next_flip = now + self.period;
            return changed;
        }
        if now < self.next_flip {
            return false;
        }
        self.visible = !self.visible;
        self.next_flip = now + self.period;
        true
    }

    /// When the timer next needs a wakeup, or `None` while it is idle.
    pub(crate) fn deadline(&self, active: bool) -> Option<Instant> {
        active.then_some(self.next_flip)
    }
}

// ========================================================================
// App: text blink
// ========================================================================

impl App {
    /// Whether blinking text needs its timer: the last frame drew some, and
    /// the window has focus (a background window holds it shown, as it does
    /// the cursor).
    pub(super) fn is_text_blink_active(&self) -> bool {
        self.window_focused
            && self
                .renderer
                .as_ref()
                .is_some_and(|renderer| renderer.drew_blinking_text())
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const PERIOD: Duration = Duration::from_millis(100);

    #[test]
    fn test_an_active_timer_flips_once_per_period() {
        let start = Instant::now();
        let mut timer = BlinkTimer::new(PERIOD, start);
        assert!(!timer.tick(start + PERIOD / 2, true), "not yet");
        assert!(timer.tick(start + PERIOD, true));
        assert!(!timer.is_visible());
        assert!(timer.tick(start + PERIOD * 2, true));
        assert!(timer.is_visible());
    }

    #[test]
    fn test_an_idle_timer_holds_shown_and_asks_for_no_wakeup() {
        let start = Instant::now();
        let mut timer = BlinkTimer::new(PERIOD, start);
        timer.tick(start + PERIOD, true);
        assert!(
            timer.tick(start + PERIOD * 10, false),
            "snaps back to shown"
        );
        assert!(timer.is_visible());
        assert_eq!(timer.deadline(false), None);
        assert!(
            !timer.tick(start + PERIOD * 10 + PERIOD / 2, true),
            "resumes a full period after going idle"
        );
    }
}
