//! The buffer list (`C-x C-b`): the terminal tabs, then every editor's
//! buffer, buried ones first, to go to or bring back into the focused pane.

use std::collections::HashSet;

use crate::model::layout::TabId;
use crate::model::picker::{Picker, PickerEntry};

use crate::window::editor::buffers::{buffer_label, tab_icon};
use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// What a row's action starts with: a terminal's tab, or an editor's buffer
/// by the tab it is or was last shown in.
const TAB_ROW: &str = "tab:";
pub(in crate::window) const BUFFER_ROW: &str = "buffer:";

/// The hint a buried buffer's row carries.
pub(in crate::window) const BURIED_HINT: &str = "buried";

// ========================================================================
// App: the buffer list
// ========================================================================

impl App {
    /// Open the buffer list: every terminal tab in pane order, then the buried
    /// buffers, the most recently buried first, then the buffers shown.
    pub(in crate::window) fn open_buffer_list(&mut self) {
        let mut rows = Vec::new();
        let mut shown = Vec::new();
        let mut listed = HashSet::new();
        for (pane_index, tabs) in self.layout().pane_members().into_iter().enumerate() {
            let pane = format!("pane {}", pane_index + 1);
            for tab in tabs {
                let buffer = self.open_tabs.editor(&tab).map(|slot| slot.view.buffer);
                match buffer {
                    // One row per buffer, however many tabs show it.
                    Some(buffer) if !listed.insert(buffer) => {}
                    Some(_) => shown.extend(self.buffer_row(tab, &pane)),
                    None => rows.push(
                        PickerEntry::new(format!("{TAB_ROW}{}", tab.0), self.tab_title(tab), pane.clone())
                            .with_icon(self.tab_icon_kind(tab)),
                    ),
                }
            }
        }
        rows.extend(self.buried_rows());
        rows.extend(shown);
        self.open_picker(Picker::open_buffer_list(rows));
        self.dirty = true;
    }

    /// Act on a row of the buffer list: go to a terminal's tab, or show a
    /// buffer in the focused pane.
    pub(in crate::window) fn choose_buffer_row(&mut self, action: &str) {
        let tab = action
            .strip_prefix(TAB_ROW)
            .or_else(|| action.strip_prefix(BUFFER_ROW))
            .and_then(|id| id.parse::<u64>().ok());
        if let Some(tab) = tab {
            self.show_buffer(TabId(tab));
        }
    }

    /// One row per buried buffer, the most recently buried first.
    pub(in crate::window) fn buried_rows(&self) -> Vec<PickerEntry> {
        self.open_tabs
            .buried()
            .iter()
            .rev()
            .filter_map(|tab| self.buffer_row(*tab, BURIED_HINT))
            .collect()
    }

    /// Show the buffer of tab `tab` in the focused pane and focus it: the
    /// pane's own tab on it where it has one, the buried buffer brought back,
    /// else a new view of a buffer only other panes show. A pane never has
    /// two tabs on one buffer. A terminal's tab is gone to wherever it is.
    pub(in crate::window) fn show_buffer(&mut self, tab: TabId) {
        if !self.open_tabs.is_editor(&tab) {
            self.switch_to_tab(tab);
            return;
        }
        if !self.open_tabs.unbury(tab) {
            self.show_view_here(tab);
            return;
        }
        self.layout_mut().add_tab(tab);
        let outcome = self
            .open_tabs
            .editor_mut(&tab)
            .map(|slot| slot.editor.on_resume());
        if let Some(outcome) = outcome {
            self.act_on_editor_outcome(tab, outcome);
        }
        self.after_tab_change();
    }

    /// Go to the focused pane's tab on the buffer tab `tab` shows, or open a
    /// view of it there.
    fn show_view_here(&mut self, tab: TabId) {
        let pane = self.layout().tabs_in_pane(self.layout().focused());
        let here = self
            .open_tabs
            .views_of(&tab)
            .into_iter()
            .find(|view| pane.contains(view));
        if let Some(here) = here {
            self.switch_to_tab(here);
            return;
        }
        let view = self.alloc_tab_id();
        self.open_tabs.add_view(view, &tab);
        self.layout_mut().add_tab(view);
        self.after_tab_change();
    }

    /// The row naming the buffer of editor tab `tab`, with `hint` at its right.
    fn buffer_row(&self, tab: TabId, hint: &str) -> Option<PickerEntry> {
        let slot = self.open_tabs.editor(&tab)?;
        let label = buffer_label(slot.kind(), slot.editor.as_ref());
        let row = PickerEntry::new(format!("{BUFFER_ROW}{}", tab.0), label, hint)
            .with_icon(tab_icon(slot.kind(), slot.editor.as_ref()));
        Some(row)
    }
}
