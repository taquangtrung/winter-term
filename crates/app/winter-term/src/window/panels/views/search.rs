//! The Search view, the bottom panel's: the latest search, the lines a
//! project search found or the rows a picker saved. A new search replaces
//! it; `t` keeps it as a saved list in the main area. Opening a line shows
//! the place it names in the main area.

use misty::view::{Space, column, text};
use misty::{Element, HostedView};

use winter_model::editor::{EditorOutcome, Notice};

use super::{ListAct, ListRow, PanelList, PanelView, RowIcon, list_view};
use crate::editors::list::place_of;
use crate::window::notice::WindowNotice;

// ========================================================================
// Constants
// ========================================================================

/// What the view is called.
const TITLE: &str = "Search";

/// What it says before any search, and over one that found nothing.
const NOTHING_KEPT: &str = "No search yet: a project search, or a picker's rows saved, shows here";
const EMPTY_SET: &str = "Nothing found";

/// What a line's place is cut from the rest of it by, as a saved list
/// writes it.
const FIELD_GAP: &str = "  ";

/// What cuts a place's path from its line number.
const PLACE_LINE: char = ':';

/// What cuts a path's folders from its file's name, as a saved list writes
/// it.
const PATH_SEPARATOR: char = '/';

/// What the heading says to the pointer: how a search outlives the next.
const KEEP_HINT: &str = "A new search replaces this one; t keeps it as a saved list";

// ========================================================================
// Data Structures
// ========================================================================

/// The latest search.
#[derive(Debug, Default)]
pub(in crate::window) struct SearchView {
    list: PanelList,
    notice: Option<WindowNotice>,
    /// The search to open in the main area as a list, until the window
    /// takes it.
    opened: Option<ResultSet>,
    /// The search shown, once there is one.
    set: Option<ResultSet>,
}

/// One search's results: what found them, and their lines.
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
    /// Show `set` in place of the search before it, its first line under
    /// the cursor.
    pub(in crate::window) fn push(&mut self, set: ResultSet) {
        let rows = set
            .lines
            .iter()
            .enumerate()
            .map(|(key, line)| line_row(key, line))
            .collect();
        self.set = Some(set);
        self.list.set_rows(rows);
        self.list.select_key(0, false);
    }

    /// The search shown, once there is one.
    pub(in crate::window) fn latest(&self) -> Option<&ResultSet> {
        self.set.as_ref()
    }

    /// Show nothing, as before any search.
    pub(in crate::window) fn clear(&mut self) {
        self.set = None;
        self.list.set_rows(Vec::new());
    }

    /// The search to open in the main area as a list, taken once.
    pub(in crate::window) fn take_opened(&mut self) -> Option<ResultSet> {
        self.opened.take()
    }

    /// The heading over the search: what found it, and what it looked for.
    pub(in crate::window) fn heading(&self) -> Option<String> {
        let set = self.set.as_ref()?;
        Some(match set.query.trim() {
            "" => set.name.clone(),
            query => format!("{}: {query}", set.name),
        })
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
                    .and_then(|row| self.set.as_ref()?.lines.get(row.key));
                match line.and_then(|line| place_of(line)) {
                    Some(target) => return EditorOutcome::OpenPath(target),
                    None => self.notice = Some(WindowNotice::NoPlace),
                }
            }
            ListAct::OpenTool => self.opened = self.set.clone(),
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

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take().map(Notice::from)
    }
}

impl HostedView for SearchView {
    fn view(&self) -> Element<'_, Self> {
        let Some(heading) = self.heading() else {
            return list_view(self, NOTHING_KEPT);
        };
        let head = Element::from(text(heading).muted().emphasized().proportional())
            .padding_px(8.0, 4.0)
            .tooltip(KEEP_HINT);
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
    let icon = place
        .as_deref()
        .map(|place| RowIcon::of_path(file_of(place), None));
    ListRow {
        detail: place,
        icon,
        key,
        label: said,
        tooltip: Some(line.to_string()),
        ..ListRow::default()
    }
}

/// The name of the file a line's place (`~/src/a.rs:3`) is in.
fn file_of(place: &str) -> &str {
    let path = place.split(PLACE_LINE).next().unwrap_or(place);
    path.rsplit(PATH_SEPARATOR).next().unwrap_or(path)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn set(name: &str, lines: &[&str]) -> ResultSet {
        ResultSet {
            lines: lines.iter().map(|line| line.to_string()).collect(),
            name: name.to_string(),
            query: String::new(),
        }
    }

    #[test]
    fn test_a_new_search_replaces_the_last_and_t_keeps_it_as_a_list() {
        let mut view = SearchView::default();
        view.push(set("Search", &["~/a.rs:3  let a", "~/b.rs:9  let b"]));
        view.push(set("Recent", &["~/notes.md"]));
        assert_eq!(view.heading().as_deref(), Some("Recent"));
        assert_eq!(view.list.rows.len(), 1, "only the latest search's lines");
        view.push(set("Search", &["~/a.rs:3  let a", "~/b.rs:9  let b"]));
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

    #[test]
    fn test_every_control_says_what_it_is_and_lights_under_the_pointer() {
        let mut view = SearchView::default();
        view.push(set("Search", &["~/a.rs:3  let a", "~/b.rs:9  let b"]));
        let (unnamed, unlit) = super::super::tests::control_faults_in(view);
        assert!(unnamed.is_empty(), "unnamed: {unnamed:?}");
        assert!(unlit.is_empty(), "unlit under the pointer: {unlit:?}");
    }
}
