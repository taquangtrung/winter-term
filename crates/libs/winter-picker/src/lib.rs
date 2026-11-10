//! Winter's picker framework: picker sessions over sources of rows, matched
//! on a thread of their own. See `docs/picker-framework-design.md`.
//!
//! - `history`: each picker's queries, and the use of its rows.
//! - `keys`: the picker's key table, block `picker`.
//! - `list`: `ListSource`, a fixed list with one thing to do on a row.
//! - `matcher`: the rows and their scoring, on a thread of their own.
//! - [`query`]: the query, its words, and how a row scores by them.
//! - `row`: what a row shows, and the fields a query meets.
//! - `session`: one picker session over a source.
//! - `source`: the `Source` trait, and what a source declares.

#![deny(unsafe_code)]
#![deny(missing_docs)]

#[cfg(any(test, feature = "test-support"))]
pub mod harness;
mod history;
mod keys;
mod list;
mod matcher;
pub mod query;
mod row;
mod session;
mod source;

pub use history::{History, Recency, Use};
pub use keys::{PickerAct, KEYS, KEY_TABLES};
pub use list::ListSource;
pub use matcher::{MatchReply, Matcher, Shown, Tracked};
pub use row::{Field, Highlights, MatchOn, Row, RowKey};
pub use session::{
    KeyHint, MenuView, Parked, Picker, PreviewUpdate, Reply, SavedLines, SessionView, Status, Tick,
    ViewRow,
};
pub use source::{
    Action, ActionId, Batch, Feed, FilePreview, Matching, Preview, PreviewKind, Resume, Source,
    SourceKey,
};
