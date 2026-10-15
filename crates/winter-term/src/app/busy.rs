//! Which tabs are busy, and the spinner their strip title wears while they are.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::model::layout::TabId;

use super::App;

// ========================================================================
// Constants
// ========================================================================

/// How long a tab is busy before its strip shows it. Most jobs and commands
/// finish inside this, and a spinner flashed for each would only flicker.
const SPINNER_DELAY: Duration = Duration::from_millis(400);

/// How long one frame of the spinner shows.
const SPINNER_FRAME: Duration = Duration::from_millis(80);

/// The spinner's frames, a braille dot circling its cell.
const SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

// ========================================================================
// Functions
// ========================================================================

/// The glyph the spinner shows on `frame`, which may count past the last.
pub(super) fn spinner_glyph(frame: usize) -> char {
    SPINNER_FRAMES[frame % SPINNER_FRAMES.len()]
}

/// The frame the spinner is on `elapsed` after the app started, so every
/// spinning tab turns in step.
fn spinner_frame_at(elapsed: Duration) -> usize {
    (elapsed.as_millis() / SPINNER_FRAME.as_millis()) as usize % SPINNER_FRAMES.len()
}

/// The tabs that have been busy for at least [`SPINNER_DELAY`] at `now`.
fn long_busy(busy_since: &HashMap<TabId, Instant>, now: Instant) -> HashSet<TabId> {
    busy_since
        .iter()
        .filter(|(_, since)| now.saturating_duration_since(**since) >= SPINNER_DELAY)
        .map(|(tab, _)| *tab)
        .collect()
}

// ========================================================================
// App: busy tabs
// ========================================================================

impl App {
    /// Bring the busy tabs up to date with what is running at `now`: a job
    /// asked for by a tab, or a command running in its shell. Marks the frame
    /// dirty when a spinner starts, stops, or turns, and returns when the next
    /// turn is due while any is showing.
    pub(crate) fn update_spinners(&mut self, now: Instant) -> Option<Instant> {
        let mut busy = self.jobs.busy_tabs();
        busy.extend(
            self.open_tabs
                .terminals()
                .filter(|(_, terminal)| terminal.command_running())
                .map(|(tab, _)| *tab),
        );
        self.busy_since.retain(|tab, _| busy.contains(tab));
        for tab in busy {
            self.busy_since.entry(tab).or_insert(now);
        }
        let spinning = long_busy(&self.busy_since, now);
        let frame = spinner_frame_at(now.saturating_duration_since(self.started));
        if spinning != self.spinning || (!spinning.is_empty() && frame != self.spinner_frame) {
            self.dirty = true;
        }
        self.spinning = spinning;
        self.spinner_frame = frame;
        match self.spinning.is_empty() {
            true => self
                .busy_since
                .values()
                .min()
                .map(|since| *since + SPINNER_DELAY),
            false => Some(now + SPINNER_FRAME),
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_tab_busy_only_briefly_does_not_spin() {
        // A refresh that answers at once is not worth a flicker in the strip.
        let now = Instant::now();
        let busy = HashMap::from([
            (TabId(1), now - Duration::from_millis(100)),
            (TabId(2), now - Duration::from_millis(900)),
        ]);
        assert_eq!(long_busy(&busy, now), HashSet::from([TabId(2)]));
    }

    #[test]
    fn test_the_spinner_turns_through_every_frame_and_wraps() {
        let seen: HashSet<char> = (0..SPINNER_FRAMES.len())
            .map(|step| spinner_frame_at(SPINNER_FRAME * step as u32))
            .map(spinner_glyph)
            .collect();
        assert_eq!(seen.len(), SPINNER_FRAMES.len());
        assert_eq!(
            spinner_frame_at(SPINNER_FRAME * SPINNER_FRAMES.len() as u32),
            0
        );
    }

    #[test]
    fn test_a_spinning_tab_wears_the_spinner_before_its_title() {
        let mut app = App::new();
        let tab = app.layout().focused();
        let title = app.tab_title(tab);
        assert_eq!(app.strip_title(tab), title);
        app.spinning.insert(tab);
        app.spinner_frame = 2;
        assert_eq!(
            app.strip_title(tab),
            format!("{} {title}", spinner_glyph(2))
        );
    }
}
