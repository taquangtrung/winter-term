//! The Search view, the bottom panel's: the lines a project search found
//! and the rows a picker saved, a result set at a time, newest first.
//! `]` and `[` go to the next and previous set; opening a line shows the
//! place it names in the main area, and `t` opens the set there as a list.

use misty::view::{Space, column, text};
use misty::{Element, HostedView};

use winter_model::editor::{EditorOutcome, Notice};
use winter_model::input::Key;
use winter_model::input::table::{self, Binding, KeyTable};

use super::{ListAct, ListRow, PanelList, PanelView, list_view};
use crate::editors::list::place_of;

// ========================================================================
// Constants
// ========================================================================

/// The Search view's own keys, block `panel_search`, ahead of the panel
/// keys.
pub(super) const KEYS: KeyTable<SearchAct> = KeyTable {
    block: "panel_search",
    bindings: &[
        Binding {
            action: SearchAct::Older,
            id: "older_set",
            keys: &["]"],
            label: "Older Result Set",
        },
        Binding {
            action: SearchAct::Newer,
            id: "newer_set",
            keys: &["["],
            label: "Newer Result Set",
        },
    ],
    configurable: true,
    title: "Search View",
};

/// What the view is called.
const TITLE: &str = "Search";

/// What it says before anything is kept, and over an empty set.
const NOTHING_KEPT: &str = "No searches kept yet: save a picker's rows to see them here";
const EMPTY_SET: &str = "Nothing in this set";

/// What a line's place is cut from the rest of it by, as a saved list
/// writes it.
const FIELD_GAP: &str = "  ";

/// What `RET` says on a line naming no place on this machine.
const NO_PLACE: &str = "No place on this line";

/// How many sets are kept, the oldest let go past it.
const MAX_SETS: usize = 20;

// ========================================================================
// Data Structures
// ========================================================================

/// What a Search view key does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) enum SearchAct {
    Newer,
    Older,
}

/// The result sets kept, and the one shown.
#[derive(Debug, Default)]
pub(in crate::window) struct SearchView {
    list: PanelList,
    notice: Option<Notice>,
    /// The set to open in the main area as a list, until the window takes
    /// it.
    opened: Option<ResultSet>,
    /// The sets, newest first.
    sets: Vec<ResultSet>,
    /// Which set is shown.
    shown: usize,
}

/// One result set: what found it, and its lines.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::window) struct ResultSet {
    pub(in crate::window) lines: Vec<String>,
    /// The picker or search that found it.
    pub(in crate::window) name: String,
    /// What its rows were filtered by.
    pub(in crate::window) query: String,
}

// ========================================================================
// SearchView Implementations
// ========================================================================

impl SearchView {
    /// Keep `set` as the newest, and show it.
    pub(in crate::window) fn push(&mut self, set: ResultSet) {
        self.sets.insert(0, set);
        self.sets.truncate(MAX_SETS);
        self.show(0);
    }

    /// The set to open in the main area as a list, taken once.
    pub(in crate::window) fn take_opened(&mut self) -> Option<ResultSet> {
        self.opened.take()
    }

    /// Show the set at `at`, its first line under the cursor.
    fn show(&mut self, at: usize) {
        self.shown = at.min(self.sets.len().saturating_sub(1));
        let rows = match self.sets.get(self.shown) {
            Some(set) => set
                .lines
                .iter()
                .enumerate()
                .map(|(key, line)| line_row(key, line))
                .collect(),
            None => Vec::new(),
        };
        self.list.set_rows(rows);
        self.list.select_key(0, false);
    }

    /// The heading over the set shown: what found it, and which of the sets
    /// it is.
    pub(in crate::window) fn heading(&self) -> Option<String> {
        let set = self.sets.get(self.shown)?;
        let name = match set.query.trim() {
            "" => set.name.clone(),
            query => format!("{}: {query}", set.name),
        };
        Some(format!(
            "{name} ({} of {})",
            self.shown + 1,
            self.sets.len()
        ))
    }
}

impl PanelView for SearchView {
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
                let line = self
                    .list
                    .selected_row()
                    .and_then(|row| self.sets.get(self.shown)?.lines.get(row.key));
                match line.and_then(|line| place_of(line)) {
                    Some(target) => return EditorOutcome::OpenPath(target),
                    None => self.notice = Some(Notice::refusal(NO_PLACE)),
                }
            }
            ListAct::OpenTool => self.opened = self.sets.get(self.shown).cloned(),
            ListAct::Collapse
            | ListAct::Down
            | ListAct::Expand
            | ListAct::First
            | ListAct::Last
            | ListAct::Refresh
            | ListAct::Up => {}
        }
        EditorOutcome::Consumed
    }

    fn own_key(&mut self, key: &Key) -> Option<EditorOutcome> {
        let act = table::find(&KEYS, key)?;
        let at = match act {
            SearchAct::Older => self.shown + 1,
            SearchAct::Newer => self.shown.saturating_sub(1),
        };
        self.show(at);
        Some(EditorOutcome::Consumed)
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }
}

impl HostedView for SearchView {
    fn view(&self) -> Element<'_, Self> {
        let Some(heading) = self.heading() else {
            return list_view(self, NOTHING_KEPT);
        };
        let head = Element::from(text(heading).muted().emphasized().proportional())
            .padding_px(8.0, 4.0)
            .tooltip("Older and newer sets: ] and [");
        column([head, list_view(self, EMPTY_SET)]).gap(Space::Px(2.0))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The row of `line`, the `key`th of its set: what it says, and the place
/// it names at its right.
fn line_row(key: usize, line: &str) -> ListRow {
    let (place, said) = match line.split_once(FIELD_GAP) {
        Some((place, said)) => (Some(place.trim().to_string()), said.trim().to_string()),
        None => (None, line.trim().to_string()),
    };
    ListRow {
        detail: place,
        key,
        label: said,
        tooltip: Some(line.to_string()),
        ..ListRow::default()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_model::input::KeyCode;

    use super::*;

    fn set(name: &str, lines: &[&str]) -> ResultSet {
        ResultSet {
            lines: lines.iter().map(|line| line.to_string()).collect(),
            name: name.to_string(),
            query: String::new(),
        }
    }

    #[test]
    fn test_the_newest_set_shows_and_brackets_step_through_older_ones() {
        let mut view = SearchView::default();
        view.push(set("Search", &["~/a.rs:3  let a", "~/b.rs:9  let b"]));
        view.push(set("Recent", &["~/notes.md"]));
        assert_eq!(view.heading().as_deref(), Some("Recent (1 of 2)"));
        view.own_key(&Key {
            alt: false,
            code: KeyCode::Char(']'),
            ctrl: false,
            shift: false,
            sup: false,
        });
        assert_eq!(view.heading().as_deref(), Some("Search (2 of 2)"));
        let rows: Vec<(String, Option<String>)> = view
            .list
            .rows
            .iter()
            .map(|row| (row.label.clone(), row.detail.clone()))
            .collect();
        assert_eq!(rows[0], ("let a".to_string(), Some("~/a.rs:3".to_string())));
        view.act(ListAct::OpenTool);
        assert_eq!(
            view.take_opened().map(|set| set.name),
            Some("Search".to_string())
        );
    }
}
