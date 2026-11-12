//! Which tabs are busy, and so wear a spinner on their strip while they are.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use winter_model::layout::TabId;

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

/// Which tabs are busy and for how long.
#[derive(Default)]
pub(in crate::window) struct Busy {
    /// When each tab that is doing something last started: a job running for
    /// it, or a command in its shell. Drives the strip's spinner.
    pub(in crate::window) since: HashMap<TabId, Instant>,
    /// The tabs busy long enough to show a spinner, which misty's strips
    /// turn.
    pub(in crate::window) spinning: HashSet<TabId>,
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
    /// dirty when a spinner starts or stops, and returns when the next tab
    /// busy so far will have been busy long enough to show one.
    pub(in crate::window) fn update_spinners(&mut self, now: Instant) -> Option<Instant> {
        let mut busy = self.host.jobs.busy_tabs();
        busy.extend(
            self.open_tabs
                .pages()
                .filter(|(_, slot)| slot.page.is_busy())
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
        if spinning != self.busy.spinning {
            self.dirty = true;
        }
        self.busy.spinning = spinning;
        self.busy
            .since
            .iter()
            .filter(|(tab, _)| !self.busy.spinning.contains(tab))
            .map(|(_, since)| *since + SPINNER_DELAY)
            .min()
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
}
