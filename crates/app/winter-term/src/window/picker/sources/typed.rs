//! A picker whose query is the answer: a build's command line, a workspace's
//! name, a mux session to start. There are no rows; `Enter` hands the query
//! on.

use misty::picker::{Action, ActionId, Feed, Matching, Resume, RowKey, Source};

use crate::window::picker::Preview;
use crate::window::picker::WindowOutcome;

// ========================================================================
// Data Structures
// ========================================================================

/// A question answered by typing.
pub(in crate::window) struct Typed {
    answer: fn(&str) -> Option<WindowOutcome>,
    id: &'static str,
    /// What the empty line says: the shape of the answer.
    placeholder: String,
    title: String,
}

// ========================================================================
// Typed
// ========================================================================

impl Typed {
    /// A question named `id`, headed by `title`, its typed answer doing what
    /// `answer` makes of it.
    pub(in crate::window) fn new(
        id: &'static str,
        title: impl Into<String>,
        answer: fn(&str) -> Option<WindowOutcome>,
    ) -> Self {
        Self {
            answer,
            id,
            placeholder: String::new(),
            title: title.into(),
        }
    }

    /// The same question, its empty line saying `text`.
    pub(in crate::window) fn placeholder(self, text: impl Into<String>) -> Self {
        Self {
            placeholder: text.into(),
            ..self
        }
    }
}

impl Source<WindowOutcome, Preview> for Typed {
    fn id(&self) -> &'static str {
        self.id
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn placeholder(&self) -> String {
        self.placeholder.clone()
    }

    fn matching(&self) -> Matching {
        Matching::Answer
    }

    fn resume(&self) -> Resume {
        Resume::Never
    }

    fn feed(&mut self, _query: &str) -> Feed {
        Feed::Fixed(Vec::new())
    }

    fn actions(&self, _row: RowKey) -> Vec<Action> {
        Vec::new()
    }

    fn run(&mut self, _action: ActionId, _row: RowKey, _query: &str) -> Option<WindowOutcome> {
        None
    }

    fn answer(&mut self, query: &str) -> Option<WindowOutcome> {
        (self.answer)(query)
    }
}
