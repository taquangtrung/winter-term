//! Buffers: the editors a window has open, held apart from the tabs that
//! show them. A tab on an editor is a view: which buffer it shows, and what
//! the host keeps of that tab's last frame. The buffer is the editor itself,
//! its kind, and when it next wants a tick.
//!
//! Several tabs may show one buffer, each a view with its own cursor and
//! scroll. The editor holds one view's state at a time, the loaded one; the
//! others keep a copy of theirs, swapped in when their tab is next acted on.

use std::any::Any;
use std::collections::HashMap;
use std::time::Instant;

use winter_core::Grid;

use crate::editors::text::DIRTY_MARK;
use crate::model::editor::Editor;
use crate::model::layout::TabId;
use crate::model::paint::{IconKind, Point};

use super::column::RowCursor;
use super::EditorKind;

// ========================================================================
// Data Structures
// ========================================================================

/// Which buffer, among a window's.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::window) struct BufferId(u64);

/// One editor and what goes with it whichever tab shows it.
pub(in crate::window) struct Buffer {
    pub(in crate::window) editor: Box<dyn Editor>,
    kind: EditorKind,
    /// The tab whose view state the editor holds now.
    pub(in crate::window) loaded: Option<TabId>,
    /// The earliest tick the editor has asked for since its last one. Kept
    /// rather than asked for anew, since an editor reckons from its own clock: a
    /// tick wanted "at once" or "a moment from now", asked for again on every
    /// pass, would stay just ahead of the loop for ever.
    pub(in crate::window) tick_at: Option<Instant>,
}

/// Every buffer of a window, by id.
#[derive(Default)]
pub(in crate::window) struct Buffers {
    buffers: HashMap<BufferId, Buffer>,
    next_id: u64,
}

/// What an editor tab keeps of its own: the buffer it shows, and the host's
/// state of the tab's last frame.
pub(in crate::window) struct EditorView {
    pub(in crate::window) buffer: BufferId,
    /// The column cursor on the editor's cursor row.
    pub(in crate::window) column: RowCursor,
    /// The row the editor last banded as its cursor line, so a text cursor
    /// starts where the user was already looking.
    pub(in crate::window) cursor_line: Option<usize>,
    /// The screen rows the last frame skipped, which a click on the pane is
    /// counted past before the editor hears of it.
    pub(in crate::window) skipped: ScreenSkip,
    /// The grid the editor was last painted into, retained so that selecting
    /// over the tab reads what is on screen: an editor has no terminal grid of
    /// its own to resolve a selection against.
    pub(in crate::window) painted: Option<Grid>,
    /// Whether rows wider than the pane wrap onto the next, for this tab
    /// alone: `M-z` turns it off and on again where it is pressed.
    pub(in crate::window) wrap: bool,
    /// This view's state, kept here while another view of the buffer is
    /// loaded in its editor; `None` while this one is, or before it ever was.
    pub(in crate::window) state: Option<Box<dyn Any>>,
}

/// Where a frame left screen rows of one of its rows unpainted: from which
/// screen row of the pane the rest come up by how many. An editor counts a
/// click's row as though nothing were skipped, so the host puts them back.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::window) struct ScreenSkip {
    /// The pane's screen row the skipped row paints from.
    from: usize,
    /// How many of its screen rows are not painted.
    rows: usize,
}

/// An editor tab as read: its view, and the editor of the buffer it shows.
pub(in crate::window) struct EditorRef<'a> {
    // The box, not the editor in it: callers reach the editor through
    // `as_ref`, and narrowing this to `&dyn Editor` changes every one.
    #[allow(clippy::borrowed_box)]
    pub(in crate::window) editor: &'a Box<dyn Editor>,
    pub(in crate::window) kind: EditorKind,
    pub(in crate::window) tick_at: Option<Instant>,
    pub(in crate::window) view: &'a EditorView,
}

/// An editor tab to change: its view, and the editor of the buffer it shows.
pub(in crate::window) struct EditorMut<'a> {
    pub(in crate::window) editor: &'a mut Box<dyn Editor>,
    pub(in crate::window) kind: EditorKind,
    pub(in crate::window) tick_at: &'a mut Option<Instant>,
    pub(in crate::window) view: &'a mut EditorView,
}

// ========================================================================
// Buffers
// ========================================================================

impl Buffers {
    /// Hold `editor`, of `kind`, as a new buffer.
    pub(in crate::window) fn insert(
        &mut self,
        kind: EditorKind,
        editor: Box<dyn Editor>,
    ) -> BufferId {
        let id = BufferId(self.next_id);
        self.next_id += 1;
        let buffer = Buffer {
            editor,
            kind,
            loaded: None,
            tick_at: None,
        };
        self.buffers.insert(id, buffer);
        id
    }

    pub(in crate::window) fn get(&self, id: BufferId) -> Option<&Buffer> {
        self.buffers.get(&id)
    }

    pub(in crate::window) fn get_mut(&mut self, id: BufferId) -> Option<&mut Buffer> {
        self.buffers.get_mut(&id)
    }

    /// Let buffer `id` go, handing it back.
    pub(in crate::window) fn remove(&mut self, id: BufferId) -> Option<Buffer> {
        self.buffers.remove(&id)
    }

    /// Every buffer, to change, in no particular order.
    pub(in crate::window) fn iter_mut(&mut self) -> impl Iterator<Item = (BufferId, &mut Buffer)> {
        self.buffers.iter_mut().map(|(id, buffer)| (*id, buffer))
    }
}

// ========================================================================
// EditorView
// ========================================================================

impl EditorView {
    /// A view of `buffer` that has not been painted yet.
    pub(in crate::window) fn new(buffer: BufferId) -> Self {
        Self {
            buffer,
            column: RowCursor::default(),
            cursor_line: None,
            painted: None,
            skipped: ScreenSkip::default(),
            state: None,
            wrap: true,
        }
    }

    /// Set the view's column cursor.
    pub(in crate::window) fn with_column(mut self, column: RowCursor) -> Self {
        self.column = column;
        self
    }

    /// Another view of the same buffer as `self`, wrapping as it does.
    pub(in crate::window) fn another(&self) -> Self {
        Self {
            wrap: self.wrap,
            ..Self::new(self.buffer)
        }
    }
}

// ========================================================================
// ScreenSkip
// ========================================================================

impl ScreenSkip {
    /// `rows` screen rows skipped of the row painting from screen row
    /// `from`; none where that row is not painted at all.
    pub(in crate::window) fn new(from: Option<usize>, rows: usize) -> Self {
        match from {
            Some(from) => Self { from, rows },
            None => Self::default(),
        }
    }

    /// `at`, a cell of the pane, as the editor counts it.
    pub(in crate::window) fn editor_point(&self, at: Point) -> Point {
        match at.row >= self.from {
            true => Point {
                row: at.row + self.rows,
                ..at
            },
            false => at,
        }
    }
}

// ========================================================================
// EditorRef
// ========================================================================

impl<'a> EditorRef<'a> {
    /// `view` of `buffer`.
    pub(in crate::window) fn new(view: &'a EditorView, buffer: &'a Buffer) -> Self {
        Self {
            editor: &buffer.editor,
            kind: buffer.kind,
            tick_at: buffer.tick_at,
            view,
        }
    }

    /// Which kind of editor this is.
    pub(in crate::window) fn kind(&self) -> EditorKind {
        self.kind
    }

    /// The icon a pane strip draws before the tab's title.
    pub(in crate::window) fn tab_icon(&self) -> IconKind {
        tab_icon(self.kind, self.editor.as_ref())
    }

    /// How a pane strip names the tab: just the editor's title, since the icon
    /// before it already says which kind of editor it is (an editor's file
    /// type, `dir`'s folder).
    pub(in crate::window) fn tab_label(&self) -> String {
        self.editor.title()
    }
}

// ========================================================================
// EditorMut
// ========================================================================

impl<'a> EditorMut<'a> {
    /// `view` of `buffer`, to change.
    pub(in crate::window) fn new(view: &'a mut EditorView, buffer: &'a mut Buffer) -> Self {
        Self {
            editor: &mut buffer.editor,
            kind: buffer.kind,
            tick_at: &mut buffer.tick_at,
            view,
        }
    }

    /// Which kind of editor this is.
    pub(in crate::window) fn kind(&self) -> EditorKind {
        self.kind
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// The icon an editor's tab draws: what the editor says it is showing (an
/// editor's file), else its kind's own.
pub(in crate::window) fn tab_icon(kind: EditorKind, editor: &dyn Editor) -> IconKind {
    editor
        .icon()
        .unwrap_or(IconKind::Editor { kind: kind.name() })
}

/// How a row of the buffer list names one buffer: its kind, what it is
/// showing, a mark when it is holding edits that are on no disk, and the
/// directory it is looking at. The directory is what tells two files of the
/// same name apart, and what makes either findable by the tree it sits in.
pub(in crate::window) fn buffer_label(kind: EditorKind, editor: &dyn Editor) -> String {
    let mut label = editor_label(kind, &editor.title());
    if editor.is_dirty() {
        label.push_str(DIRTY_MARK);
    }
    if let Some(dir) = editor.cwd() {
        label.push_str("  ");
        label.push_str(&dir.display().to_string());
    }
    label
}

/// How an editor is named away from the editor itself: the kind it was opened
/// as and what it is showing. A title that merely restates the kind's name
/// ("Keys") is left to stand on its own rather than doubled up.
fn editor_label(kind: EditorKind, title: &str) -> String {
    let name = kind.name();
    let named = title
        .get(..name.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(name));
    match named {
        true => title.to_string(),
        false => format!("{name}: {title}"),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_click_below_skipped_screen_rows_is_counted_past_them() {
        // Two screen rows of the row painting from screen row one skipped: a
        // click on the heading above stays, one on the row or below it is
        // what the editor laid out two rows further down.
        let skip = ScreenSkip::new(Some(1), 2);
        let at = |row| Point { col: 3, drag: false, row };
        assert_eq!(skip.editor_point(at(0)), at(0));
        assert_eq!(skip.editor_point(at(1)), at(3));
        assert_eq!(skip.editor_point(at(4)), at(6));
        assert_eq!(ScreenSkip::new(None, 2).editor_point(at(4)), at(4), "nothing skipped where the row is not painted");
    }
}
