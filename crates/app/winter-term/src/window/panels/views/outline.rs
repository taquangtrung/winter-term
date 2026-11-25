//! The Outline view, a sidebar's: the headings and symbols of the main tab
//! last worked in, one to a row, set in as deep as each is nested, its line
//! at the right. The window hands it the outline as the tab changes, and
//! the entry the tab's cursor is in is selected while the view does not
//! have the keyboard. Opening an entry takes the tab there; `l` and `h`
//! open and close an entry holding others.
//!
//! The outline is read again only when the tab says it changed (its
//! [`outline_revision`](winter_model::editor::Page::outline_revision)
//! moved), once the changes pause for [`SETTLE`], so typing does not read
//! it at every key. Following the tab's cursor needs no read.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use misty::{Element, HostedView};

use winter_model::editor::{EditorOutcome, JumpTarget};
use winter_model::layout::TabId;

use super::{ListAct, ListRow, PanelList, PanelView, RowIcon, list_view};

// ========================================================================
// Constants
// ========================================================================

/// What the view is called.
const TITLE: &str = "Outline";

/// What it says with nothing to list.
const EMPTY: &str = "No outline in this tab";

/// How long the tab's outline must stay unchanged before it is read again.
pub(in crate::window) const SETTLE: Duration = Duration::from_millis(300);

/// What follows an entry's line number in its label, as an editor leads
/// it with one.
const LINE_GAP: &str = "  ";

// ========================================================================
// Data Structures
// ========================================================================

/// The outline of the main tab last worked in.
#[derive(Debug, Default)]
pub(in crate::window) struct OutlineView {
    /// The entry chosen to go to, by the tab's own index for it, until the
    /// window takes it.
    chosen: Option<usize>,
    /// The entries closed, the ones nested in them out of the list, by
    /// their index.
    closed: HashSet<usize>,
    /// The tab the outline was read from, and its revision then.
    read_from: Option<(TabId, Option<u64>)>,
    /// The newest revision of the tab seen, waited out before a read.
    seen: Option<u64>,
    /// When the changes seen will have paused long enough to read.
    settle_at: Option<Instant>,
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
            if source != self.source {
                self.closed.clear();
            }
            self.entries = entries;
            self.source = source;
            self.relist();
        }
        if let Some(at) = at.filter(|at| *at < self.entries.len()) {
            let shown = self.listed_ancestor(at);
            self.list.select_key(shown, false);
        }
    }

    /// Whether the outline of tab `source`, now at `revision`, is to be read
    /// again at `now`: at once for another tab, and for a change once the
    /// changes have paused for [`SETTLE`].
    pub(in crate::window) fn is_due(
        &mut self,
        source: TabId,
        revision: Option<u64>,
        now: Instant,
    ) -> bool {
        let Some((tab, read)) = self.read_from else {
            return true;
        };
        if tab != source {
            return true;
        }
        if read == revision {
            self.settle_at = None;
            return false;
        }
        if self.seen != revision {
            self.seen = revision;
            self.settle_at = Some(now + SETTLE);
        }
        self.settle_at.is_some_and(|at| now >= at)
    }

    /// Note the outline shown as read from tab `source` at `revision`.
    pub(in crate::window) fn mark_read(&mut self, source: TabId, revision: Option<u64>) {
        self.read_from = Some((source, revision));
        self.seen = revision;
        self.settle_at = None;
    }

    /// Select the entry the tab's cursor, on its index `cursor`, is in: the
    /// last starting at or before it, or the listed entry holding that.
    pub(in crate::window) fn follow_cursor(&mut self, cursor: usize) {
        let Some(at) = self
            .entries
            .iter()
            .rposition(|entry| entry.target <= cursor)
        else {
            return;
        };
        let shown = self.listed_ancestor(at);
        if self.list.selected_row().map(|row| row.key) != Some(shown) {
            self.list.select_key(shown, false);
        }
    }

    /// List the entries, leaving out those nested in a closed one.
    fn relist(&mut self) {
        let mut rows = Vec::new();
        let mut hidden_below: Option<usize> = None;
        for (key, entry) in self.entries.iter().enumerate() {
            if hidden_below.is_some_and(|depth| entry.depth > depth) {
                continue;
            }
            hidden_below = None;
            let fold = self.holds_others(key).then(|| !self.closed.contains(&key));
            if fold == Some(false) {
                hidden_below = Some(entry.depth);
            }
            rows.push(ListRow {
                fold,
                ..entry_row(key, entry)
            });
        }
        self.list.set_rows(rows);
    }

    /// Whether the entry at `at` has others nested in it.
    fn holds_others(&self, at: usize) -> bool {
        let depth = self.entries.get(at).map(|entry| entry.depth);
        let next = self.entries.get(at + 1).map(|entry| entry.depth);
        matches!((depth, next), (Some(depth), Some(next)) if next > depth)
    }

    /// The entry the one at `at` is nested in, if any.
    fn parent_of(&self, at: usize) -> Option<usize> {
        let depth = self.entries.get(at)?.depth;
        self.entries[..at]
            .iter()
            .rposition(|entry| entry.depth < depth)
    }

    /// The entry at `at`, or the nearest entry holding it that is listed.
    fn listed_ancestor(&self, at: usize) -> usize {
        let mut shown = at;
        for ancestor in std::iter::successors(self.parent_of(at), |up| self.parent_of(*up)) {
            if self.closed.contains(&ancestor) {
                shown = ancestor;
            }
        }
        shown
    }

    /// Open or close the entry the cursor is on; closing one holding none,
    /// or one closed, goes to the entry it is nested in.
    fn fold(&mut self, open: bool) {
        let Some(at) = self.list.selected_row().map(|row| row.key) else {
            return;
        };
        let closed = self.closed.contains(&at);
        match (open, self.holds_others(at) && !closed) {
            (true, _) if closed => {
                self.closed.remove(&at);
                self.relist();
            }
            (false, true) => {
                self.closed.insert(at);
                self.relist();
            }
            (false, false) => {
                if let Some(parent) = self.parent_of(at) {
                    self.list.select_key(parent, false);
                }
            }
            (true, _) => {}
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

    /// Only while changes to the outline are being waited out.
    fn next_tick(&self) -> Option<Instant> {
        self.settle_at
    }

    /// The changes have paused: the frame this asks for reads the outline.
    fn on_tick(&mut self) -> EditorOutcome {
        EditorOutcome::Consumed
    }

    fn set_shown(&mut self, shown: bool) -> EditorOutcome {
        if !shown {
            self.settle_at = None;
        }
        EditorOutcome::Consumed
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
            ListAct::Collapse => {
                self.fold(false);
                EditorOutcome::Consumed
            }
            ListAct::Expand => {
                self.fold(true);
                EditorOutcome::Consumed
            }
            ListAct::Down
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

/// The row of `entry`, the `key`th: its words set in as deep as it is
/// nested, the line number leading its label at the right.
fn entry_row(key: usize, entry: &JumpTarget) -> ListRow {
    let label = entry.label.trim_start();
    let numbered = label
        .split_once(LINE_GAP)
        .filter(|(line, _)| line.chars().all(|c| c.is_ascii_digit()));
    let (words, line) = match numbered {
        Some((line, words)) => (words.trim_start(), Some(line.to_string())),
        None => (label, None),
    };
    ListRow {
        depth: entry.depth,
        detail: line,
        icon: entry.icon.clone().map(RowIcon::of_kind),
        key,
        label: words.to_string(),
        tooltip: Some(words.to_string()),
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
        let depth = label.len() - label.trim_start().len();
        JumpTarget::new(target, label.trim_start().to_string()).with_depth(depth / 2)
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

    #[test]
    fn test_every_control_says_what_it_is_and_lights_under_the_pointer() {
        let mut view = OutlineView::default();
        view.set_outline(
            Some("notes.md".to_string()),
            vec![entry("Intro", 3), entry("  Usage", 40)],
            None,
        );
        let (unnamed, unlit) = super::super::tests::control_faults_in(view);
        assert!(unnamed.is_empty(), "unnamed: {unnamed:?}");
        assert!(unlit.is_empty(), "unlit under the pointer: {unlit:?}");
    }

    #[test]
    fn test_an_entry_holding_others_closes_and_opens_and_the_cursor_climbs_out() {
        let mut view = OutlineView::default();
        let outline = vec![
            JumpTarget::new(0, "    1  Intro".to_string()),
            JumpTarget::new(4, "    5    Usage".to_string()).with_depth(1),
            JumpTarget::new(9, "   10      Keys".to_string()).with_depth(2),
            JumpTarget::new(20, "   21  End".to_string()),
        ];
        view.set_outline(Some("notes.md".to_string()), outline, Some(2));
        let rows = |view: &OutlineView| -> Vec<(usize, String, Option<String>)> {
            let rows = view.list.rows.iter();
            rows.map(|row| (row.depth, row.label.clone(), row.detail.clone()))
                .collect()
        };
        assert_eq!(
            rows(&view)[2],
            (2, "Keys".to_string(), Some("10".to_string())),
            "nested, its line at the right"
        );
        view.act(ListAct::Collapse);
        assert_eq!(
            view.list.selected_row().map(|row| row.key),
            Some(1),
            "out to Usage"
        );
        view.act(ListAct::Collapse);
        assert_eq!(rows(&view).len(), 3, "Keys out of the list");
        view.set_outline(Some("notes.md".to_string()), view.entries.clone(), Some(2));
        assert_eq!(
            view.list.selected_row().map(|row| row.key),
            Some(1),
            "the cursor's entry hidden, its listed parent chosen"
        );
        view.act(ListAct::Expand);
        assert_eq!(rows(&view).len(), 4, "open again");
    }

    #[test]
    fn test_a_changed_outline_is_read_once_the_changes_pause_and_never_on_a_timer() {
        let mut view = OutlineView::default();
        let (tab, other) = (TabId(1), TabId(2));
        let now = Instant::now();
        assert!(view.is_due(tab, Some(1), now), "never read: read at once");
        view.mark_read(tab, Some(1));
        assert!(!view.is_due(tab, Some(1), now), "unchanged");
        assert_eq!(view.next_tick(), None, "quiet: no tick");

        assert!(!view.is_due(tab, Some(2), now), "changed: waited out");
        assert_eq!(view.next_tick(), Some(now + SETTLE));
        let later = now + SETTLE / 2;
        assert!(!view.is_due(tab, Some(3), later), "changed again");
        assert_eq!(
            view.next_tick(),
            Some(later + SETTLE),
            "the wait starts over"
        );
        assert!(view.is_due(tab, Some(3), later + SETTLE), "paused: read");

        view.set_shown(false);
        assert_eq!(view.next_tick(), None, "out of sight: no tick");
        assert!(
            view.is_due(other, Some(3), later),
            "another tab: read at once"
        );
    }
}
