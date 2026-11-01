//! Where a picker's rows come from, and what they do: the one trait every
//! picker's source implements, and what it declares about itself.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use winter_model::input::Key;
use winter_model::paint::IconKind;

use super::row::{Highlights, MatchOn, Row, RowKey};

// ========================================================================
// Data Structures
// ========================================================================

/// Rows a source hands over in one go, and whether more come after them.
#[derive(Debug, Default)]
pub struct Batch {
    /// Whether the source stopped at its cap with rows left unsent.
    pub capped: bool,
    /// Whether these are the last rows.
    pub done: bool,
    /// Why the source stopped, when it failed: the picker keeps the rows it
    /// has and says this in its header.
    pub error: Option<String>,
    /// The rows.
    pub rows: Vec<Row>,
    /// The row the picker starts selected on, unless the selection was
    /// moved already: the file the browser was opened at.
    pub select: Option<RowKey>,
}

/// How a source's rows come.
#[derive(Debug)]
pub enum Feed {
    /// The source cannot start; the reason shows in place of rows.
    Failed(String),
    /// All of them at once.
    Fixed(Vec<Row>),
    /// In batches from work of the source's own, until one is done.
    Stream(Receiver<Batch>),
}

/// How a source's rows meet the query.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Matching {
    /// The query is the answer, a name or a command line typed: there are no
    /// rows to match.
    Answer,
    /// The source matches, giving rows again for each query once the typing
    /// pauses; the fields named are where it highlights.
    BySource(MatchOn),
    /// The picker matches by words, on the fields named.
    Words(MatchOn),
}

/// What resuming a source's picker session does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resume {
    /// A search: its rows are kept, and it goes on past them where it
    /// stopped early.
    Continue,
    /// The session cannot be resumed: it answered a question.
    Never,
    /// A listing: its rows are read again, so nothing gone since is shown.
    Reread,
}

/// A source's kind of preview.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewKind {
    /// A live picker: the pane it is about shows the selected row's target.
    Live,
    /// No preview.
    None,
    /// A panel inside the picker shows the selected row.
    Panel,
}

/// What a preview shows. The window draws it and opens nothing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Preview {
    /// A file, centred on a line.
    File(FilePreview),
    /// A tab, by its id.
    Tab(u64),
    /// One of the pane's own places, by the index its editor gave it.
    Target(usize),
    /// Text to show as it is.
    Text(String),
}

/// A file to preview, and the place in it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilePreview {
    /// The column of the match, from one, when known.
    pub column: Option<usize>,
    /// The line to centre on, from one.
    pub line: usize,
    /// The file.
    pub path: PathBuf,
}

/// A source action's id, unique within its source: what its key table binds.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ActionId(pub &'static str);

/// One thing a row can do, as the action menu and the row's buttons show it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Action {
    /// The icon the row shows for it under the pointer, when it is a button.
    pub button: Option<IconKind>,
    /// What it is bound and run by.
    pub id: ActionId,
    /// What the menu calls it.
    pub title: String,
}

/// What a source made of a key it took.
#[derive(Debug, Eq, PartialEq)]
pub enum SourceKey<O> {
    /// The key chose something: carry out the outcome and close.
    Done(O),
    /// The key changed what the source lists, its rows to be given again.
    Refeed,
    /// The key moved the source somewhere else (the browser stepping into a
    /// directory): the query is cleared and the rows given again.
    Reset,
    /// The key changed nothing the picker shows.
    Stay,
}

// ========================================================================
// Traits
// ========================================================================

/// Where a picker's rows come from, and what choosing one does. `O` is what
/// the host carries out.
pub trait Source<O>: Send {
    /// The picker's name: what its history, its recency, and its key table
    /// are kept under.
    fn id(&self) -> &'static str;

    /// What the picker is headed by.
    fn title(&self) -> String;

    /// How the rows meet the query.
    fn matching(&self) -> Matching;

    /// What resuming the picker session does.
    fn resume(&self) -> Resume;

    /// The kind of preview the rows have.
    fn preview_kind(&self) -> PreviewKind {
        PreviewKind::None
    }

    /// Whether rows rank recently used first.
    fn ranks_recent(&self) -> bool {
        false
    }

    /// The name `row`'s use is remembered by when it is chosen (a command's
    /// id, a file's path), for a source whose rows rank recently used first.
    fn recency_name(&self, _row: RowKey) -> Option<String> {
        None
    }

    /// Start giving rows: once, or for `Matching::BySource` for each query
    /// once the typing pauses.
    fn feed(&mut self, query: &str) -> Feed;

    /// Where a `Matching::BySource` match is lit in `row`, for `query`.
    fn highlights(&self, _row: &Row, _query: &str) -> Highlights {
        Highlights::default()
    }

    /// What `row` can do, its default first.
    fn actions(&self, row: RowKey) -> Vec<Action>;

    /// Do `action` on `row`; `None` leaves the picker open.
    fn run(&mut self, action: ActionId, row: RowKey, query: &str) -> Option<O>;

    /// A row the query itself stands for, listed after every match whatever
    /// it matches: the browser's new file, named as typed. Its key is the
    /// source's to tell apart in `actions` and `run`.
    fn query_row(&self, _query: &str) -> Option<Row> {
        None
    }

    /// What the query typed into a `Matching::Answer` picker does.
    fn answer(&mut self, _query: &str) -> Option<O> {
        None
    }

    /// What previewing `row` shows.
    fn preview(&self, _row: RowKey) -> Option<Preview> {
        None
    }

    /// Whether the rows can be saved to a buffer.
    fn can_save(&self) -> bool {
        false
    }

    /// Get ready to be fed again on resume: a search raises its cap by a
    /// batch, so resuming shows more.
    fn refresh(&mut self) {}

    /// The rows shown, in order, saved as one outcome; `None` when the
    /// source has nothing to save.
    fn export(&self, _rows: &[RowKey], _query: &str) -> Option<O> {
        None
    }

    /// A key the source gives a meaning of its own (the browser's `Tab`, the
    /// pane digits), on the selected row, with the query as typed; `None`
    /// leaves it to the picker.
    fn key(&mut self, _key: &Key, _row: Option<RowKey>, _query: &str) -> Option<SourceKey<O>> {
        None
    }
}
