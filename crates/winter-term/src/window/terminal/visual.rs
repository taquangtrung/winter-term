//! Visual mode over the scrollback: starting and swapping the selection's
//! ends, yanking it, and remembering it for `gv`.

use crate::model::input::VisualKind;
use crate::model::layout::TabId;
use crate::model::mode::{Mode, ModeEvent};

use crate::window::{App, LastVisual};

// ========================================================================
// App: visual mode
// ========================================================================

impl App {
    /// Leave Visual mode after a yank: remember the span for `gv`, then clear
    /// it and the anchor.
    pub(in crate::window) fn finish_visual_yank(&mut self, focused: TabId) {
        // The selection's own ends are the yank's, which Vim marks `[` and `]`.
        if let Some(span) = self.selection.span.clone() {
            let (first, last) = if (span.start_row, span.start_col) <= (span.end_row, span.end_col)
            {
                (
                    (span.start_row, span.start_col),
                    (span.end_row, span.end_col),
                )
            } else {
                (
                    (span.end_row, span.end_col),
                    (span.start_row, span.start_col),
                )
            };
            self.set_auto_mark(focused, '[', first);
            self.set_auto_mark(focused, ']', last);
        }
        self.remember_visual(focused);
        self.open_tabs
            .set_mode(&focused, Mode::Visual.apply(ModeEvent::Escape));
        self.selection.visual_anchor = None;
        self.selection.span = None;
        self.dirty = true;
    }

    /// Enter Visual mode from Normal, toggle it off when the same kind is pressed
    /// again, or switch between charwise, linewise, and blockwise while staying in Visual.
    pub(in crate::window) fn toggle_visual(&mut self, kind: VisualKind, focused: TabId) {
        let mode = self.open_tabs.mode(&focused).unwrap_or_default();
        match mode {
            Mode::Normal => {
                self.open_tabs
                    .set_mode(&focused, Mode::Normal.apply(ModeEvent::EnterVisual));
                self.selection.visual_anchor =
                    Some(self.absolute_nav_cursor(focused).unwrap_or((0, 0)));
                self.selection.visual_kind = kind;
                self.update_visual_selection(focused);
            }
            Mode::Visual if self.selection.visual_kind == kind => {
                // Same kind again leaves Visual, back to Normal.
                self.remember_visual(focused);
                self.open_tabs
                    .set_mode(&focused, Mode::Visual.apply(ModeEvent::EnterVisual));
                self.selection.visual_anchor = None;
                self.selection.span = None;
            }
            Mode::Visual => {
                // Switch kind (charwise <-> linewise <-> blockwise), keeping the anchor.
                self.selection.visual_kind = kind;
                self.update_visual_selection(focused);
            }
            Mode::Insert => {}
        }
        self.dirty = true;
    }

    /// Visual `o`: put the cursor on the selection's other end (the anchor),
    /// so extending continues from there. The highlighted span is unchanged,
    /// it always runs anchor..cursor in either order.
    pub(in crate::window) fn swap_visual_ends(&mut self, focused: TabId) {
        if let (Some(anchor), Some(cursor)) = (
            self.selection.visual_anchor,
            self.absolute_nav_cursor(focused),
        ) {
            self.selection.visual_anchor = Some(cursor);
            // Both ends are absolute, so the old anchor may be off screen;
            // `reveal_position` scrolls it back into view before parking there.
            self.reveal_position(focused, anchor);
            self.update_visual_selection(focused);
            self.dirty = true;
        }
    }

    /// Snapshot the live Visual selection as `gv`'s restore target, called at
    /// every point a Visual selection ends. Both ends are stored in absolute
    /// coordinates (see `winter_core::Grid::to_absolute_row`) so the snapshot
    /// still names the same text after the view has scrolled or grown; a pane
    /// without a live selection (no anchor or cursor) snapshots nothing.
    pub(in crate::window) fn remember_visual(&mut self, focused: TabId) {
        let Some(anchor) = self.selection.visual_anchor else {
            return;
        };
        let Some((cursor_row, cursor_col)) = self.nav_cursor(focused) else {
            return;
        };
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let grid = terminal.grid();
        self.selection.last_visual = Some(LastVisual {
            anchor,
            cursor: (grid.to_absolute_row(cursor_row), cursor_col),
            kind: self.selection.visual_kind,
            tab: focused,
        });
    }

    /// `gv`: re-enter Visual mode with the last selection `remember_visual`
    /// captured: same pane, same kind, same two ends. The cursor end is
    /// revealed (centered if it scrolled off screen); the anchor end is clamped
    /// to the viewport when the span no longer fits, exactly like vim's own
    /// `gv` on a selection taller than the window.
    pub(in crate::window) fn restore_visual(&mut self, focused: TabId) {
        let Some(last) = self
            .selection
            .last_visual
            .as_ref()
            .filter(|lv| lv.tab == focused)
            .copied()
        else {
            return;
        };
        self.open_tabs
            .set_mode(&focused, Mode::Normal.apply(ModeEvent::EnterVisual));
        self.selection.visual_kind = last.kind;
        self.reveal_position(focused, last.cursor);
        self.selection.visual_anchor = Some(last.anchor);
        self.update_visual_selection(focused);
        self.dirty = true;
    }
}
