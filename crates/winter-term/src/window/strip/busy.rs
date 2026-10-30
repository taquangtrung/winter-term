//! Which tabs are busy, and the spinner their strip title wears while they are.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::model::layout::TabId;
use crate::model::spinner;

use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// How long a tab is busy before its strip shows it. Most jobs and commands
/// finish inside this, and a spinner flashed for each would only flicker.
const SPINNER_DELAY: Duration = Duration::from_millis(400);

// ========================================================================
// Data Structures
// ========================================================================

/// Which tabs are busy and for how long, and where the strip spinner is in
/// its turn.
#[derive(Default)]
pub(in crate::window) struct Busy {
    /// When each tab that is doing something last started: a job running for
    /// it, or a command in its shell. Drives the strip's spinner.
    pub(in crate::window) since: HashMap<TabId, Instant>,
    /// The tabs busy long enough to show a spinner, and the frame it is on.
    pub(in crate::window) spinning: HashSet<TabId>,
    pub(in crate::window) frame: usize,
}

// ========================================================================
// Functions
// ========================================================================

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
    pub(in crate::window) fn update_spinners(&mut self, now: Instant) -> Option<Instant> {
        let mut busy = self.host.jobs.busy_tabs();
        busy.extend(
            self.open_tabs
                .editors()
                .filter(|(_, slot)| slot.editor.is_busy())
                .map(|(tab, _)| *tab),
        );
        busy.extend(
            self.open_tabs
                .terminals()
                .filter(|(_, terminal)| terminal.command_running())
                .map(|(tab, _)| *tab),
        );
        self.busy.since.retain(|tab, _| busy.contains(tab));
        for tab in busy {
            self.busy.since.entry(tab).or_insert(now);
        }
        let spinning = long_busy(&self.busy.since, now);
        // Counted from the app's start, so every spinning tab turns in step.
        let frame = spinner::frame_at(now.saturating_duration_since(self.runtime.started));
        if spinning != self.busy.spinning || (!spinning.is_empty() && frame != self.busy.frame) {
            self.dirty = true;
        }
        self.busy.spinning = spinning;
        self.busy.frame = frame;
        match self.busy.spinning.is_empty() {
            true => self
                .busy
                .since
                .values()
                .min()
                .map(|since| *since + SPINNER_DELAY),
            false => Some(now + spinner::FRAME),
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
    fn test_a_spinning_tab_wears_the_spinner_before_its_title() {
        let mut app = App::new();
        let tab = app.layout().focused();
        let title = app.tab_title(tab);
        assert_eq!(app.strip_title(tab), title);
        app.busy.spinning.insert(tab);
        app.busy.frame = 2;
        assert_eq!(
            app.strip_title(tab),
            format!("{} {title}", spinner::glyph(2))
        );
    }
}
