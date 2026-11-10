//! Typing into every pane at once, as tmux's `synchronize-panes` does: while
//! it is on, what is typed or pasted into the focused terminal goes to every
//! other terminal the panes show too, the same bytes as each would have had
//! typed into it. An editor's pane, and a terminal in a tab behind another,
//! takes none of it. The status bar says so while it is on.

use winter_model::layout::TabId;

use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// What the status bar's mode wears while typing goes to every pane.
pub(in crate::window) const BROADCAST_MARK: &str = " [ALL PANES]";

// ========================================================================
// App: typing into every pane
// ========================================================================

impl App {
    /// Start or stop sending what is typed into the focused terminal to
    /// every terminal shown.
    pub(in crate::window) fn toggle_broadcast(&mut self) {
        self.term.broadcast = !self.term.broadcast;
        let said = match self.term.broadcast {
            true => "typing goes to every terminal shown",
            false => "typing goes to the focused terminal alone",
        };
        self.set_unlogged_notice(said);
        self.dirty = true;
    }

    /// The terminals besides `from` that typing into it goes to as well:
    /// every one the panes show, while typing goes to them all.
    pub(in crate::window) fn broadcast_targets(&self, from: TabId) -> Vec<TabId> {
        if !self.term.broadcast {
            return Vec::new();
        }
        self.layout()
            .shown_tabs()
            .into_iter()
            .filter(|tab| *tab != from && self.open_tabs.is_terminal(tab))
            .collect()
    }

    /// Send `bytes`, typed into `from`, to every other terminal shown.
    pub(in crate::window) fn broadcast_bytes(&mut self, from: TabId, bytes: &[u8]) {
        for tab in self.broadcast_targets(from) {
            if let Some(terminal) = self.open_tabs.terminal_mut(&tab) {
                terminal.write(bytes);
            }
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::test_support::{app_with_tabs, idle_terminal};
    use winter_model::layout::Direction;

    #[test]
    fn test_typing_goes_to_the_other_shown_terminals_only_while_it_is_on() {
        // A pane showing tab 0 with tab 1 behind it, and a second pane.
        let mut app = app_with_tabs(2);
        app.split_pane(Direction::Vertical);
        let shown = app.layout().shown_tabs();
        assert_eq!(shown.len(), 2);
        for tab in shown.iter().chain([&TabId(1)]) {
            app.open_tabs.insert_terminal(*tab, idle_terminal());
        }
        let focused = app.layout().focused();
        assert!(app.broadcast_targets(focused).is_empty(), "off by default");

        app.toggle_broadcast();
        let other: Vec<TabId> = shown.into_iter().filter(|tab| *tab != focused).collect();
        assert_eq!(other.len(), 1);
        assert_eq!(
            app.broadcast_targets(focused),
            other,
            "the shown one, not the one behind"
        );

        app.toggle_broadcast();
        assert!(app.broadcast_targets(focused).is_empty(), "off again");
    }
}
