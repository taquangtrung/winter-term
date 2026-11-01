//! A two-phase blink clock: shown, then hidden, flipping every period while
//! something on screen is blinking, and holding shown otherwise.

use std::time::{Duration, Instant};

use crate::window::{App, CURSOR_BLINK_PERIOD};

// ========================================================================
// Constants
// ========================================================================

/// How long blinking text (SGR 5) spends in each phase. Slower than the
/// cursor, as in xterm, so text flashing beside a blinking cursor reads as
/// a separate signal.
pub(in crate::window) const TEXT_BLINK_PERIOD: Duration = Duration::from_millis(800);

// ========================================================================
// Data Structures
// ========================================================================

/// The two blink clocks: the cursor's, reset by every key, and blinking
/// text's, which runs on its own period.
pub(in crate::window) struct Blinking {
    /// Whether the cursor is in its "visible" blink phase. Toggled by the blink
    /// timer; always `true` while blink is disabled.
    pub(in crate::window) phase: bool,
    /// When the next blink phase flip is due. Updated on every flip and on key
    /// press (which resets the cursor to visible to give feedback).
    pub(in crate::window) next_flip: Instant,
    /// The phase of blinking text (SGR 5): its own clock, running only while
    /// the last frame drew some and the window has focus, whatever the
    /// cursor's blink setting.
    pub(in crate::window) text: BlinkTimer,
}

/// A blink phase and when it next flips.
#[derive(Clone, Copy, Debug)]
pub(in crate::window) struct BlinkTimer {
    next_flip: Instant,
    period: Duration,
    visible: bool,
}

// ========================================================================
// Blinking Implementations
// ========================================================================

impl Blinking {
    /// Both clocks starting at `now`, the cursor shown.
    pub(in crate::window) fn new(now: Instant) -> Self {
        Self {
            phase: true,
            next_flip: now + CURSOR_BLINK_PERIOD,
            text: BlinkTimer::new(TEXT_BLINK_PERIOD, now),
        }
    }
}

// ========================================================================
// BlinkTimer
// ========================================================================

impl BlinkTimer {
    /// A timer in its shown phase, first flipping one `period` from `now`.
    pub(in crate::window) fn new(period: Duration, now: Instant) -> Self {
        Self {
            next_flip: now + period,
            period,
            visible: true,
        }
    }

    /// Whether the blinking thing is in its shown phase.
    pub(in crate::window) fn is_visible(&self) -> bool {
        self.visible
    }

    /// Advance to `now`. While `active`, flip once the period is up; while
    /// not, hold the shown phase and restart the period, so blinking resumes
    /// from shown rather than mid-cycle. Returns whether the phase changed.
    pub(in crate::window) fn tick(&mut self, now: Instant, active: bool) -> bool {
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
    pub(in crate::window) fn deadline(&self, active: bool) -> Option<Instant> {
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
    pub(in crate::window) fn is_text_blink_active(&self) -> bool {
        self.input.focus.window
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
