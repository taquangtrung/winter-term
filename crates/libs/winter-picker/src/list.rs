//! The one-call way to build a picker that is a fixed list with one thing to
//! do on a row: the panes, Open With, an editor's question, an extension's
//! list.

use super::row::{MatchOn, Row, RowKey};
use super::source::{Action, ActionId, Feed, Matching, Resume, Source};

// ========================================================================
// Constants
// ========================================================================

/// The one action of a list, run by `Enter`.
const CHOOSE: ActionId = ActionId("choose");

/// What the action menu calls it.
const CHOOSE_TITLE: &str = "Choose";

// ========================================================================
// Data Structures
// ========================================================================

/// A fixed list whose rows each do one thing.
pub struct ListSource<O> {
    choose: Box<dyn FnMut(RowKey) -> Option<O> + Send>,
    id: &'static str,
    match_on: MatchOn,
    rows: Option<Vec<Row>>,
    title: String,
}

// ========================================================================
// ListSource
// ========================================================================

impl<O> ListSource<O> {
    /// A list named `id`, headed by `title`, of `rows`, where choosing a row
    /// does what `choose` returns for its key.
    pub fn new(
        id: &'static str,
        title: impl Into<String>,
        rows: Vec<Row>,
        choose: impl FnMut(RowKey) -> Option<O> + Send + 'static,
    ) -> Self {
        Self {
            choose: Box::new(choose),
            id,
            match_on: MatchOn::LABEL,
            rows: Some(rows),
            title: title.into(),
        }
    }

    /// The same list, its query matched against the fields `on` names.
    pub fn match_on(self, on: MatchOn) -> Self {
        Self {
            match_on: on,
            ..self
        }
    }
}

impl<O> Source<O> for ListSource<O> {
    fn id(&self) -> &'static str {
        self.id
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn matching(&self) -> Matching {
        Matching::Words(self.match_on)
    }

    fn resume(&self) -> Resume {
        Resume::Never
    }

    fn feed(&mut self, _query: &str) -> Feed {
        Feed::Fixed(self.rows.take().unwrap_or_default())
    }

    fn actions(&self, _row: RowKey) -> Vec<Action> {
        vec![Action {
            button: None,
            id: CHOOSE,
            title: CHOOSE_TITLE.to_string(),
        }]
    }

    fn run(&mut self, _action: ActionId, row: RowKey, _query: &str) -> Option<O> {
        (self.choose)(row)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_list_source_runs_its_one_action_on_the_chosen_row() {
        let rows = vec![Row::new(RowKey(1), "one"), Row::new(RowKey(2), "two")];
        let mut list = ListSource::new("panes", "Panes", rows, |key| Some(key.0 * 10));
        let actions = list.actions(RowKey(2));
        assert_eq!(actions.len(), 1);
        assert_eq!(list.run(actions[0].id, RowKey(2), ""), Some(20));
    }
}
