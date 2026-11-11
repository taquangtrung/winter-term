//! An extension's picker: rows the extension gives when it opens it and
//! pushes after, each doing the actions the extension named, its choice and
//! (for a picker that hears it) each settled query handed back to it.
//!
//! The rows travel through a [`PickerLink`] the window and the source share:
//! the window pushes into it what the extension gives, on its thread, and
//! takes from it the queries the extension is to hear.

use std::collections::HashSet;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use misty::picker::{
    Action, ActionId, Batch, Feed, MatchOn, Matching, PreviewKind, Resume, Row, RowKey, Source,
};
use winter_base::wake::wake_window;
use winter_ext_api::{PickerId, PickerRow};
use winter_model::paint::IconKind;

use crate::window::picker::sources::IconRow;
use crate::window::picker::WindowOutcome;
use crate::window::picker::{FilePreview, Preview};

// ========================================================================
// Constants
// ========================================================================

/// The action of a row the extension named none for.
const CHOOSE: &str = "choose";

/// What the action menu calls it.
const CHOOSE_TITLE: &str = "Choose";

// ========================================================================
// Data Structures
// ========================================================================

/// An extension's picker.
pub(in crate::window) struct ExtensionSource {
    hears_query: bool,
    id: PickerId,
    link: PickerLink,
    title: String,
}

/// What the window and an extension's picker share: the rows given so far,
/// where more go, and the query waiting to be heard.
#[derive(Clone, Default)]
pub(in crate::window) struct PickerLink(Arc<Mutex<LinkState>>);

#[derive(Default)]
struct LinkState {
    /// Where pushed rows go, while the picker is open.
    feed: Option<Sender<Batch>>,
    /// The action ids the extension named, kept for the session: an action
    /// holds its id as a static name, so each is kept once.
    names: HashSet<&'static str>,
    /// The query the extension has yet to hear.
    query: Option<String>,
    /// The rows given since the picker opened, or since the last query,
    /// keyed by their places.
    rows: Vec<PickerRow>,
    /// Whether more rows come after the first.
    streamed: bool,
}

/// What an extension's picker chose: which picker, which action, which row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct ExtensionChoice {
    pub(in crate::window) action: String,
    pub(in crate::window) picker: PickerId,
    pub(in crate::window) row: usize,
}

// ========================================================================
// ExtensionSource
// ========================================================================

impl ExtensionSource {
    /// The picker `id`, headed by `title`, its rows through `link`.
    pub(in crate::window) fn new(
        id: PickerId,
        title: String,
        hears_query: bool,
        link: PickerLink,
    ) -> Self {
        Self {
            hears_query,
            id,
            link,
            title,
        }
    }

    fn row(&self, row: RowKey) -> Option<PickerRow> {
        let state = self.link.lock();
        state.rows.get(usize::try_from(row.0).ok()?).cloned()
    }
}

impl Source<WindowOutcome, Preview> for ExtensionSource {
    fn id(&self) -> &'static str {
        "extension"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn matching(&self) -> Matching {
        match self.hears_query {
            true => Matching::BySource(MatchOn::LABEL),
            false => Matching::Words(MatchOn::LABEL | MatchOn::DESCRIPTION),
        }
    }

    fn resume(&self) -> Resume {
        Resume::Never
    }

    fn preview_kind(&self) -> PreviewKind {
        PreviewKind::Panel
    }

    /// Open the line rows come down: the rows given so far down it at once,
    /// or, for a picker that hears the query, none, the query waiting for
    /// the extension to give rows for.
    fn feed(&mut self, query: &str) -> Feed {
        let (send, batches) = mpsc::channel();
        let mut state = self.link.lock();
        match self.hears_query {
            true => {
                state.rows.clear();
                state.query = Some(query.to_string());
                wake_window();
            }
            false => {
                let rows = rows_from(&state.rows, 0);
                let _ = send.send(Batch {
                    done: !state.streamed,
                    rows,
                    ..Batch::default()
                });
            }
        }
        state.feed = Some(send);
        Feed::Stream(batches)
    }

    fn actions(&self, row: RowKey) -> Vec<Action> {
        let Some(row) = self.row(row) else {
            return Vec::new();
        };
        let mut state = self.link.lock();
        let mut actions: Vec<Action> = row
            .actions
            .iter()
            .map(|action| Action {
                button: None,
                id: ActionId(state.name(&action.id)),
                title: action.title.clone(),
            })
            .collect();
        if actions.is_empty() {
            actions.push(Action {
                button: None,
                id: ActionId(CHOOSE),
                title: CHOOSE_TITLE.to_string(),
            });
        }
        actions
    }

    fn run(&mut self, action: ActionId, row: RowKey, _query: &str) -> Option<WindowOutcome> {
        Some(WindowOutcome::ExtensionChosen(ExtensionChoice {
            action: action.0.to_string(),
            picker: self.id,
            row: usize::try_from(row.0).ok()?,
        }))
    }

    fn preview(&self, row: RowKey) -> Option<Preview> {
        let row = self.row(row)?;
        if let Some(text) = row.preview {
            return Some(Preview::Text(text));
        }
        Some(Preview::File(FilePreview {
            column: None,
            line: 1,
            path: row.file?,
        }))
    }
}

// ========================================================================
// PickerLink
// ========================================================================

impl PickerLink {
    /// A link starting with `rows`, `streamed` when more come after.
    pub(in crate::window) fn new(rows: Vec<PickerRow>, streamed: bool) -> Self {
        Self(Arc::new(Mutex::new(LinkState {
            rows,
            streamed,
            ..LinkState::default()
        })))
    }

    /// Give the picker `rows` after those it has; `done` once no more come.
    pub(in crate::window) fn push(&self, rows: Vec<PickerRow>, done: bool) {
        let mut state = self.lock();
        let first = state.rows.len();
        let shown = rows_from(&rows, first);
        state.rows.extend(rows);
        if let Some(feed) = &state.feed {
            let _ = feed.send(Batch {
                done,
                rows: shown,
                ..Batch::default()
            });
        }
    }

    /// The query the extension has yet to hear, taken.
    pub(in crate::window) fn take_query(&self) -> Option<String> {
        self.lock().query.take()
    }

    /// Whether the picker is gone, the window alone holding the link.
    pub(in crate::window) fn is_closed(&self) -> bool {
        Arc::strong_count(&self.0) == 1
    }

    fn lock(&self) -> MutexGuard<'_, LinkState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

// ========================================================================
// LinkState
// ========================================================================

impl LinkState {
    /// The action id `id` as a static name, made once and kept.
    fn name(&mut self, id: &str) -> &'static str {
        if let Some(name) = self.names.get(id) {
            return name;
        }
        let name: &'static str = Box::leak(id.to_string().into_boxed_str());
        self.names.insert(name);
        name
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The picker's rows of `rows`, keyed by their places from `first`.
fn rows_from(rows: &[PickerRow], first: usize) -> Vec<Row> {
    rows.iter()
        .zip(first..)
        .map(|(row, at)| {
            let mut shown = Row::new(RowKey(at as u64), row.label.clone());
            if let Some(description) = &row.description {
                shown = shown.with_description(description.clone());
            }
            if let Some(detail) = &row.detail {
                shown = shown.with_detail(detail.clone());
            }
            let name = row
                .file
                .as_deref()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned());
            match name {
                Some(name) => shown.with_icon_kind(IconKind::File { name }),
                None => shown,
            }
        })
        .collect()
}
