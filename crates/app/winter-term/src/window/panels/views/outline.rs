//! The Outline view, a sidebar's: the headings and symbols of the main tab
//! last worked in, one to a row. The window hands it the outline as the
//! tab changes, and the entry the tab's cursor is in is selected while the
//! view does not have the keyboard. Opening an entry takes the tab there.

use misty::{Element, HostedView};

use winter_model::editor::{EditorOutcome, JumpTarget};

use super::{ListAct, ListRow, PanelList, PanelView, list_view};

// ========================================================================
// Constants
// ========================================================================

/// What the view is called.
const TITLE: &str = "Outline";

/// What it says with nothing to list.
const EMPTY: &str = "No outline in this tab";

/// How many spaces of an entry's label make one level of depth.
const SPACES_PER_LEVEL: usize = 2;

// ========================================================================
// Data Structures
// ========================================================================

/// The outline of the main tab last worked in.
#[derive(Debug, Default)]
pub(in crate::window) struct OutlineView {
    /// The entry chosen to go to, by the tab's own index for it, until the
    /// window takes it.
    chosen: Option<usize>,
    /// The tab's headings and symbols, in its order.
    entries: Vec<JumpTarget>,
    list: PanelList,
    /// The name of the tab outlined, `None` with none.
    source: Option<String>,
}

// ========================================================================
// OutlineView Implementations
// ========================================================================

impl OutlineView {
    /// Show `entries`, the outline of the tab named `source`, selecting `at`,
    /// the entry its cursor is in, when one is given.
    pub(in crate::window) fn set_outline(
        &mut self,
        source: Option<String>,
        entries: Vec<JumpTarget>,
        at: Option<usize>,
    ) {
        let same = source == self.source && entries == self.entries;
        if !same {
            let rows = entries
                .iter()
                .enumerate()
                .map(|(key, entry)| entry_row(key, entry));
            self.list.set_rows(rows.collect());
            self.entries = entries;
            self.source = source;
        }
        if let Some(at) = at {
            self.list.select_key(at, false);
        }
    }

    /// The entry chosen to go to, by the tab's own index for it, taken so it
    /// is gone to once.
    pub(in crate::window) fn take_chosen(&mut self) -> Option<usize> {
        self.chosen.take()
    }
}

impl PanelView for OutlineView {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn list(&self) -> &PanelList {
        &self.list
    }

    fn list_mut(&mut self) -> &mut PanelList {
        &mut self.list
    }

    fn act(&mut self, act: ListAct) -> EditorOutcome {
        match act {
            ListAct::Open => {
                self.chosen = self
                    .list
                    .selected_row()
                    .and_then(|row| self.entries.get(row.key))
                    .map(|entry| entry.target);
                EditorOutcome::Consumed
            }
            ListAct::Collapse
            | ListAct::Down
            | ListAct::Expand
            | ListAct::First
            | ListAct::Last
            | ListAct::OpenTool
            | ListAct::Refresh
            | ListAct::Up => EditorOutcome::Consumed,
        }
    }
}

impl HostedView for OutlineView {
    fn view(&self) -> Element<'_, Self> {
        list_view(self, EMPTY)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The row of `entry`, the `key`th: its label set in as deep as its leading
/// spaces say.
fn entry_row(key: usize, entry: &JumpTarget) -> ListRow {
    let label = entry.label.trim_start();
    let depth = (entry.label.len() - label.len()) / SPACES_PER_LEVEL;
    ListRow {
        depth,
        key,
        label: label.to_string(),
        tooltip: Some(label.to_string()),
        ..ListRow::default()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(label: &str, target: usize) -> JumpTarget {
        JumpTarget {
            icon: None,
            label: label.to_string(),
            target,
        }
    }

    #[test]
    fn test_opening_an_entry_chooses_its_place_in_the_tab_once() {
        let mut view = OutlineView::default();
        view.set_outline(
            Some("notes.md".to_string()),
            vec![entry("Intro", 3), entry("  Usage", 40)],
            Some(1),
        );
        assert_eq!(
            view.list.selected_row().map(|row| row.depth),
            Some(1),
            "set in"
        );
        view.act(ListAct::Open);
        assert_eq!(view.take_chosen(), Some(40));
        assert_eq!(view.take_chosen(), None, "taken once");
    }
}
