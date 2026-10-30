//! The keys of the focused tab (`<f1> h`): an editor's own, else the
//! window's.

use crate::model::layout::TabId;
use crate::model::picker::{Picker, PickerEntry};
use crate::window::commands::builtin_commands;

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
        let (title, rows) = match self.open_tabs.editor(&focused) {
            Some(slot) => {
                let rows = slot
                    .editor
                    .keys()
                    .into_iter()
                    .map(|help| PickerEntry::new(String::new(), help.label, help.keys))
                    .collect();
                (format!("{KEYS_TITLE}: {}", slot.editor.title()), rows)
            }
            None => {
                let rows = builtin_commands(&self.keymap)
                    .into_iter()
                    .filter(|entry| !entry.shortcut.is_empty())
                    .collect();
                (format!("{KEYS_TITLE}: {WINDOW_KEYS}"), rows)
            }
        };
        self.open_picker(Picker::open_tab_keys(rows, title));
        self.dirty = true;
    }
}
