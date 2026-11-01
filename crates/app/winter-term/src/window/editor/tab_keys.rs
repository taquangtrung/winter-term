//! The keys of the focused tab (`F1 h`): an editor's own, else the
//! window's.

use winter_model::layout::TabId;
use winter_picker::{Resume, Row, RowKey};

use crate::window::commands::BUILTIN;
use crate::window::picker::sources::{Entry, Listing};
use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// Heads the list of a tab's keys, before what the tab is.
const KEYS_TITLE: &str = "Keys";

/// What a terminal's key list is called: its keys are the window's.
const WINDOW_KEYS: &str = "Window";

// ========================================================================
// App: the tab's keys
// ========================================================================

impl App {
    /// List the keys of the focused tab: an editor's own, read from the tables
    /// it dispatches by, else every window command bound to a key.
    pub(in crate::window) fn open_tab_keys(&mut self, focused: TabId) {
        let (title, rows): (String, Vec<Row>) = match self.open_tabs.page(&focused) {
            Some(slot) => {
                let rows = slot
                    .page
                    .keys()
                    .into_iter()
                    .map(|help| Row::new(RowKey(0), help.label).with_hint(help.keys))
                    .collect();
                (format!("{KEYS_TITLE}: {}", slot.page.title()), rows)
            }
            None => {
                let rows = BUILTIN
                    .iter()
                    .map(|builtin| (builtin, self.keymap.chord_hint(builtin.id)))
                    .filter(|(_, chord)| !chord.is_empty())
                    .map(|(builtin, chord)| Row::new(RowKey(0), builtin.title).with_hint(chord))
                    .collect();
                (format!("{KEYS_TITLE}: {WINDOW_KEYS}"), rows)
            }
        };
        // A key list is read, not chosen from.
        let mut listing = Listing::new("tab_keys", title).resume(Resume::Never);
        for row in rows {
            listing.push(Entry::read_only(row));
        }
        self.open_picker(Box::new(listing));
        self.dirty = true;
    }
}
