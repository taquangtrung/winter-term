//! A listing: a fixed list of rows the window built from what it holds, each
//! row doing what its outcomes say. Most pickers are one: the command
//! palette, the recent list, the buffers, the panes, Scout.

use winter_model::paint::IconKind;
use winter_picker::{
    Action, ActionId, Feed, MatchOn, Matching, Preview, PreviewKind, Resume, Row, RowKey, Source,
};

use crate::window::picker::WindowOutcome;

// ========================================================================
// Constants
// ========================================================================

/// The action every listing row has first, run by `Enter`.
pub(in crate::window) const CHOOSE: ListingAction = ListingAction {
    button: None,
    id: ActionId("choose"),
    title: "Choose",
};

// ========================================================================
// Data Structures
// ========================================================================

/// A fixed list of rows and what each does.
pub(in crate::window) struct Listing {
    /// The actions rows may have, in the order a row lists them.
    actions: Vec<ListingAction>,
    entries: Vec<Entry>,
    id: &'static str,
    matching: Matching,
    preview: PreviewKind,
    ranks_recent: bool,
    resume: Resume,
    /// Where the picker starts selected, when not on its first row.
    title: String,
}

/// One kind of thing a listing's rows can do.
#[derive(Clone, Debug)]
pub(in crate::window) struct ListingAction {
    /// The icon a row shows for it as a button, when it is one.
    pub(in crate::window) button: Option<IconKind>,
    pub(in crate::window) id: ActionId,
    pub(in crate::window) title: &'static str,
}

/// One row of a listing, and what it does.
pub(in crate::window) struct Entry {
    /// What each of the listing's actions does on this row, in their order;
    /// `None` where the row does not offer it. An empty list: the row is
    /// read, not chosen.
    outcomes: Vec<Option<WindowOutcome>>,
    preview: Option<Preview>,
    recency: Option<String>,
    row: Row,
}

// ========================================================================
// Listing
// ========================================================================

impl Listing {
    /// An empty listing named `id`, headed by `title`, its rows matched on
    /// their labels, read again on resume, with no preview.
    pub(in crate::window) fn new(id: &'static str, title: impl Into<String>) -> Self {
        Self {
            actions: vec![CHOOSE],
            entries: Vec::new(),
            id,
            matching: Matching::Words(MatchOn::LABEL),
            preview: PreviewKind::None,
            ranks_recent: false,
            resume: Resume::Reread,
            title: title.into(),
        }
    }

    /// The same listing, with `actions` after its first.
    pub(in crate::window) fn extra_actions(self, actions: &[ListingAction]) -> Self {
        let mut all = vec![CHOOSE];
        all.extend_from_slice(actions);
        Self {
            actions: all,
            ..self
        }
    }

    /// The same listing, its rows matched on the fields `on` names.
    pub(in crate::window) fn match_on(self, on: MatchOn) -> Self {
        Self {
            matching: Matching::Words(on),
            ..self
        }
    }

    /// The same listing, with a preview of the `kind` given.
    pub(in crate::window) fn preview(self, kind: PreviewKind) -> Self {
        Self {
            preview: kind,
            ..self
        }
    }

    /// The same listing, its rows ranked by their use.
    pub(in crate::window) fn ranks_recent(self) -> Self {
        Self {
            ranks_recent: true,
            ..self
        }
    }

    /// The same listing, resumed as `resume` says.
    pub(in crate::window) fn resume(self, resume: Resume) -> Self {
        Self { resume, ..self }
    }

    /// Add `entry`, keyed by its place.
    pub(in crate::window) fn push(&mut self, entry: Entry) {
        let key = RowKey(self.entries.len() as u64);
        self.entries.push(Entry {
            row: Row { key, ..entry.row },
            ..entry
        });
    }

    /// Whether the listing has no rows.
    pub(in crate::window) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn entry(&self, row: RowKey) -> Option<&Entry> {
        self.entries.get(usize::try_from(row.0).ok()?)
    }
}

impl Source<WindowOutcome> for Listing {
    fn id(&self) -> &'static str {
        self.id
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn matching(&self) -> Matching {
        self.matching
    }

    fn resume(&self) -> Resume {
        self.resume
    }

    fn preview_kind(&self) -> PreviewKind {
        self.preview
    }

    fn ranks_recent(&self) -> bool {
        self.ranks_recent
    }

    fn recency_name(&self, row: RowKey) -> Option<String> {
        self.entry(row)?.recency.clone()
    }

    fn feed(&mut self, _query: &str) -> Feed {
        Feed::Fixed(self.entries.iter().map(|entry| entry.row.clone()).collect())
    }

    fn actions(&self, row: RowKey) -> Vec<Action> {
        let Some(entry) = self.entry(row) else {
            return Vec::new();
        };
        self.actions
            .iter()
            .zip(&entry.outcomes)
            .filter(|(_, outcome)| outcome.is_some())
            .map(|(action, _)| Action {
                button: action.button.clone(),
                id: action.id,
                title: action.title.to_string(),
            })
            .collect()
    }

    fn run(&mut self, action: ActionId, row: RowKey, _query: &str) -> Option<WindowOutcome> {
        let entry = self.entry(row)?;
        let at = self.actions.iter().position(|kind| kind.id == action)?;
        entry.outcomes.get(at)?.clone()
    }

    fn preview(&self, row: RowKey) -> Option<Preview> {
        self.entry(row)?.preview.clone()
    }
}

// ========================================================================
// Entry
// ========================================================================

impl Entry {
    /// A row whose first action does `outcome`.
    pub(in crate::window) fn new(row: Row, outcome: WindowOutcome) -> Self {
        Self {
            outcomes: vec![Some(outcome)],
            preview: None,
            recency: None,
            row,
        }
    }

    /// A row that is read, not chosen.
    pub(in crate::window) fn read_only(row: Row) -> Self {
        Self {
            outcomes: Vec::new(),
            preview: None,
            recency: None,
            row,
        }
    }

    /// The same row, its listing's further actions doing `outcomes`, in
    /// their order, `None` where it does not offer one.
    pub(in crate::window) fn with_actions(mut self, outcomes: Vec<Option<WindowOutcome>>) -> Self {
        self.outcomes.extend(outcomes);
        self
    }

    /// The same row, previewed as `preview`.
    pub(in crate::window) fn with_preview(self, preview: Preview) -> Self {
        Self {
            preview: Some(preview),
            ..self
        }
    }

    /// The same row, its use remembered as `name`.
    pub(in crate::window) fn with_recency(self, name: impl Into<String>) -> Self {
        Self {
            recency: Some(name.into()),
            ..self
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_model::layout::TabId;

    use super::*;

    const KILL: ListingAction = ListingAction {
        button: None,
        id: ActionId("kill_buffer"),
        title: "Kill",
    };

    #[test]
    fn test_a_row_offers_only_the_actions_it_has_an_outcome_for() {
        let mut list = Listing::new("buffers", "Buffers").extra_actions(&[KILL]);
        let row = || Row::new(RowKey(0), "x");
        list.push(Entry::new(row(), WindowOutcome::FocusTab(TabId(1))));
        let kill = WindowOutcome::KillBuffer(TabId(2));
        list.push(
            Entry::new(row(), WindowOutcome::ShowBuffer(TabId(2))).with_actions(vec![Some(kill)]),
        );
        assert_eq!(
            list.actions(RowKey(0)).len(),
            1,
            "a terminal cannot be killed as a buffer"
        );
        assert_eq!(list.actions(RowKey(1)).len(), 2);
        let killed = list.run(KILL.id, RowKey(1), "");
        assert_eq!(killed, Some(WindowOutcome::KillBuffer(TabId(2))));
        assert_eq!(list.run(KILL.id, RowKey(0), ""), None);
    }
}
