//! Text selection, word selection, and clipboard copy/paste.

use crate::model::input::VisualKind;
use crate::model::layout::PaneId;

use super::super::Selection;
use super::App;

// ========================================================================
// Constants
// ========================================================================

const WORD_CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-.~/";

// ========================================================================
// Free functions
// ========================================================================

/// The status-bar message confirming a copy: line and character counts of
/// `text`, pluralized.
fn copy_confirmation(text: &str) -> String {
    let lines = text.lines().count();
    let chars = text.chars().count();
    format!(
        "Copied {lines} line{}, {chars} character{}",
        if lines == 1 { "" } else { "s" },
        if chars == 1 { "" } else { "s" },
    )
}

// ========================================================================
// App: clipboard & selection
// ========================================================================

impl App {
    /// The selected text, and its line/character counts. Rows in `self.selection.span`
    /// are absolute (see [`winter_render::Grid::to_absolute_row`]), so this reads
    /// via `absolute_cell` rather than the scroll-position-dependent `visible_cell`
    ///: a selection built up over an auto-scrolled drag stays correct however far
    /// the view has since scrolled.
    pub(crate) fn selected_text(&self) -> Option<String> {
        let sel = self.selection.span.as_ref()?;
        let pane = self.panes.get(&sel.pane)?;
        let grid = pane.grid();

        if sel.block {
            let (sr, er) = (
                sel.start_row.min(sel.end_row),
                sel.start_row.max(sel.end_row),
            );
            let (sc, ec) = (
                sel.start_col.min(sel.end_col),
                sel.start_col.max(sel.end_col),
            );
            let mut text = String::new();
            for row in sr..=er {
                let mut line = String::new();
                let col_end = (ec + 1).min(grid.cols());
                for col in sc..col_end {
                    let ch = grid.absolute_cell(row, col).map(|c| c.ch).unwrap_or(' ');
                    line.push(ch);
                }
                text.push_str(line.trim_end());
                if row < er {
                    text.push('\n');
                }
            }
            let trimmed = text.trim_end().to_string();
            return if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            };
        }

        let (sr, sc, er, ec) = if (sel.start_row, sel.start_col) <= (sel.end_row, sel.end_col) {
            (sel.start_row, sel.start_col, sel.end_row, sel.end_col)
        } else {
            (sel.end_row, sel.end_col, sel.start_row, sel.start_col)
        };

        let mut text = String::new();
        for row in sr..=er {
            let indent = grid.absolute_row_wrap_indent(row);
            let col_start = if row == sr {
                sc
            } else if grid.absolute_row_wraps(row.saturating_sub(1)) {
                indent
            } else {
                0
            };
            let col_end = if row == er { ec + 1 } else { grid.cols() };
            let col_end = col_end.min(grid.cols());
            let mut line = String::new();
            for col in col_start..col_end {
                let ch = grid.absolute_cell(row, col).map(|c| c.ch).unwrap_or(' ');
                line.push(ch);
            }
            // A span running to the row's last column swept up the blank padding
            // past the text (every row of a linewise selection does); drop it, so
            // the copied lines end where their content does.
            if col_end >= grid.cols() {
                line.truncate(line.trim_end().len());
            }
            text.push_str(&line);
            if row < er && !grid.absolute_row_wraps(row) {
                text.push('\n');
            }
        }
        let trimmed = text.trim_end().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }

    /// Recompute the highlighted selection from the Visual-mode anchor and the
    /// current nav cursor. Charwise spans anchor->cursor; linewise covers every
    /// column of the rows between them. The nav cursor's rows are viewport-
    /// relative, so both are converted to `Selection`'s absolute addressing
    /// (see [`winter_render::Grid::to_absolute_row`]) before storing.
    pub(crate) fn update_visual_selection(&mut self, focused: PaneId) {
        let (Some((abs_ar, ac)), Some((cr, cc))) =
            (self.selection.visual_anchor, self.nav_cursor(focused))
        else {
            self.selection.span = None;
            return;
        };
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let grid = pane.grid();
        let last_col = grid.cols().saturating_sub(1);
        // The anchor is already absolute; only the live cursor is a viewport row.
        let abs_cr = grid.to_absolute_row(cr);
        self.selection.span = Some(match self.selection.visual_kind {
            VisualKind::Block => Selection {
                block: true,
                end_col: cc,
                end_row: abs_cr,
                pane: focused,
                start_col: ac,
                start_row: abs_ar,
            },
            VisualKind::Line => Selection {
                block: false,
                end_col: last_col,
                end_row: abs_ar.max(abs_cr),
                pane: focused,
                start_col: 0,
                start_row: abs_ar.min(abs_cr),
            },
            VisualKind::Char => Selection {
                block: false,
                end_col: cc,
                end_row: abs_cr,
                pane: focused,
                start_col: ac,
                start_row: abs_ar,
            },
        });
    }

    /// The shared, long-lived clipboard handle, created on first use. Returns
    /// `None` if the platform clipboard is unavailable (e.g. a headless test).
    /// Reusing one handle keeps the Linux clipboard owner alive so written
    /// contents survive long enough for other apps and clipboard managers.
    pub(crate) fn clipboard(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        self.clipboard.as_mut()
    }

    /// Copy the current selection to the system clipboard and confirm it in the
    /// status bar with its line/character counts. A no-op (and no notice) when
    /// nothing is selected, so plain clicks that leave no selection stay silent.
    pub(crate) fn copy_selection(&mut self) {
        let Some(text) = self.selected_text() else {
            return;
        };
        let Some(clipboard) = self.clipboard() else {
            return;
        };
        let copied = clipboard.set_text(&text).is_ok();
        if copied {
            self.set_notice(copy_confirmation(&text));
        }
    }

    /// Copy the current selection to the X11/Wayland primary selection buffer.
    /// On non-Linux platforms this is a no-op.
    #[allow(unused_variables)]
    pub(crate) fn copy_selection_to_primary(&mut self) {
        let Some(text) = self.selected_text() else {
            return;
        };
        #[cfg(target_os = "linux")]
        {
            use arboard::{LinuxClipboardKind, SetExtLinux};
            if let Some(cb) = self.clipboard() {
                let _ = cb.set().clipboard(LinuxClipboardKind::Primary).text(text);
            }
        }
    }

    /// Paste text from the X11/Wayland primary selection buffer.
    /// On non-Linux platforms this falls back to the regular clipboard.
    pub(crate) fn paste_from_primary(&mut self) {
        let text = self.get_primary_text();
        let Some(text) = text else { return };
        let focused = self.tab().focused();
        if let Some(shadow) = self.prompt_shadows.get_mut(&focused) {
            shadow.desync();
        }
        let Some(pane) = self.panes.get_mut(&focused) else {
            return;
        };
        if pane.bracketed_paste() {
            let mut bytes = Vec::with_capacity(text.len() + 12);
            bytes.extend_from_slice(b"\x1b[200~");
            bytes.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
            pane.write(&bytes);
        } else {
            pane.write(text.as_bytes());
        }
    }

    fn get_primary_text(&mut self) -> Option<String> {
        let cb = self.clipboard()?;
        #[cfg(target_os = "linux")]
        {
            use arboard::{GetExtLinux, LinuxClipboardKind};
            cb.get().clipboard(LinuxClipboardKind::Primary).text().ok()
        }
        #[cfg(not(target_os = "linux"))]
        {
            cb.get_text().ok()
        }
    }

    pub(crate) fn clipboard_text(&mut self) -> Option<String> {
        self.clipboard().and_then(|cb| cb.get_text().ok())
    }

    pub(crate) fn paste_text(&mut self, text: &str) {
        let focused = self.tab().focused();
        if let Some(shadow) = self.prompt_shadows.get_mut(&focused) {
            shadow.desync();
        }
        let Some(pane) = self.panes.get_mut(&focused) else {
            return;
        };
        if pane.bracketed_paste() {
            let mut bytes = Vec::with_capacity(text.len() + 8);
            bytes.extend_from_slice(b"\x1b[200~");
            bytes.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
            pane.write(&bytes);
        } else {
            pane.write(text.as_bytes());
        }
    }

    pub(crate) fn paste_from_clipboard(&mut self) {
        if let Some(text) = self.clipboard_text() {
            self.paste_text(&text);
        }
    }

    /// `row` is the viewport row under the click; the resulting `Selection` is
    /// stored with its absolute row (see [`winter_render::Grid::to_absolute_row`]).
    pub(crate) fn select_word_at(&mut self, pane_id: PaneId, row: usize, col: usize) {
        let Some(pane) = self.panes.get(&pane_id) else {
            return;
        };
        let grid = pane.grid();
        let abs_row = grid.to_absolute_row(row);
        let ch = grid.cell(row, col).map(|c| c.ch).unwrap_or(' ');
        if !WORD_CHARS.contains(ch) {
            self.selection.span = Some(Selection {
                block: false,
                start_row: abs_row,
                start_col: col,
                end_row: abs_row,
                end_col: col,
                pane: pane_id,
            });
            return;
        }
        let mut start = col;
        let mut end = col;
        while start > 0 {
            if let Some(c) = grid.cell(row, start - 1).map(|c| c.ch) {
                if WORD_CHARS.contains(c) {
                    start -= 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        while end < grid.cols() - 1 {
            if let Some(c) = grid.cell(row, end + 1).map(|c| c.ch) {
                if WORD_CHARS.contains(c) {
                    end += 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        self.selection.span = Some(Selection {
            block: false,
            start_row: abs_row,
            start_col: start,
            end_row: abs_row,
            end_col: end,
            pane: pane_id,
        });
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_word_chars_contains_alphanumeric() {
        assert!(WORD_CHARS.contains('a'));
        assert!(WORD_CHARS.contains('Z'));
        assert!(WORD_CHARS.contains('0'));
        assert!(WORD_CHARS.contains('_'));
        assert!(!WORD_CHARS.contains(' '));
    }

    #[test]
    fn test_copy_confirmation_pluralizes_multiple_lines_and_chars() {
        assert_eq!(copy_confirmation("ab\ncd"), "Copied 2 lines, 5 characters");
    }

    #[test]
    fn test_copy_confirmation_singular_for_one_line_one_char() {
        assert_eq!(copy_confirmation("a"), "Copied 1 line, 1 character");
    }

    #[test]
    fn test_copy_confirmation_counts_unicode_scalars_not_bytes() {
        // Regression: counting `text.len()` (bytes) instead of `.chars()`
        // would report a multi-byte-per-char string (e.g. emoji, accented
        // Latin) as having far more "characters" than a user actually typed
        // or selected.
        assert_eq!(copy_confirmation("héllo"), "Copied 1 line, 5 characters");
    }

    /// An app whose focused pane holds `line` on its top row, in Visual mode with
    /// the anchor at `anchor` and the nav cursor at `cursor` (both viewport cols).
    fn app_with_visual_selection(line: &str, anchor: usize, cursor: usize) -> crate::app::App {
        let mut app = crate::app::App::new();
        app.config.status_bar.enabled = true;
        let id = app.tab().panes()[0];
        let mut pane = crate::terminal::pane::Pane::with_command(
            20,
            4,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        {
            let grid = pane.grid_mut();
            grid.move_to(0, 0);
            for ch in line.chars() {
                grid.print(ch);
            }
        }
        app.panes.insert(id, pane);
        app.modes.insert(id, crate::model::mode::Mode::Visual);
        app.selection.visual_anchor = Some((0, anchor));
        app.set_nav_cursor(id, (0, cursor));
        app.update_visual_selection(id);
        app
    }

    /// A pane whose grid holds `lines`, one per row, with the oldest pushed
    /// into scrollback once they overflow the viewport.
    fn app_with_scrollback(lines: &[&str], rows: usize) -> (App, PaneId) {
        let mut app = App::new();
        let id = app.tab().panes()[0];
        let mut pane = crate::terminal::pane::Pane::with_command(
            20,
            rows,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        {
            let grid = pane.grid_mut();
            for (i, line) in lines.iter().enumerate() {
                if i > 0 {
                    grid.line_feed();
                    grid.carriage_return();
                }
                for ch in line.chars() {
                    grid.print(ch);
                }
            }
        }
        app.panes.insert(id, pane);
        // Normal, not Visual: `Action::EnterVisual` toggles, so a pane already
        // in Visual would leave it instead of anchoring.
        app.modes.insert(id, crate::model::mode::Mode::Normal);
        (app, id)
    }

    #[test]
    fn test_visual_anchor_holds_its_line_when_the_cursor_scrolls() {
        // `v` anchors on the text under the cursor. Walking up past the top of
        // the viewport scrolls history, and the anchor has to keep naming the
        // line it was placed on. Held viewport-relative it slid by exactly the
        // scroll distance, so the highlight detached from where `v` was pressed.
        use crate::model::input::{Action, CursorMove, VisualKind};

        let (mut app, id) = app_with_scrollback(&["aaa", "bbb", "ccc", "ddd", "eee", "fff"], 3);
        // Top visible row is absolute row 3 ("ddd"): three lines are in scrollback.
        app.set_nav_cursor(id, (0, 0));
        app.handle_action(Action::EnterVisual(VisualKind::Char), id);
        let anchored = app.selection.span.as_ref().expect("selection").start_row;
        assert_eq!(anchored, 3, "anchor should sit on absolute row 3");

        // Each `k` from the top edge scrolls one more line of history into view.
        for _ in 0..2 {
            app.handle_action(Action::MoveCursor(CursorMove::Up), id);
        }
        assert_eq!(
            app.panes[&id].grid().scroll_offset(),
            2,
            "the motion should have scrolled history"
        );
        assert_eq!(
            app.selection.span.as_ref().expect("selection").start_row,
            anchored,
            "the anchor drifted with the viewport instead of holding its line"
        );
    }

    #[test]
    fn test_visual_selection_includes_the_character_under_the_cursor() {
        // Vim's charwise Visual is inclusive at both ends, so `v` then two `l`
        // yanks three characters: the cell the cursor sits on is part of it.
        let app = app_with_visual_selection("abcdef", 0, 2);
        assert_eq!(app.selected_text().as_deref(), Some("abc"));
    }

    #[test]
    fn test_visual_selection_backwards_also_includes_the_cursor_cell() {
        // Extending left of the anchor keeps both endpoints: cols 1..=4.
        let app = app_with_visual_selection("abcdef", 4, 1);
        assert_eq!(app.selected_text().as_deref(), Some("bcde"));
    }

    #[test]
    fn test_selected_text_joins_soft_wrapped_lines_and_skips_hanging_indent() {
        let mut app = App::new();
        let id = app.tab().panes()[0];
        let mut pane = crate::terminal::pane::Pane::with_command(
            10,
            4,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        {
            let grid = pane.grid_mut();
            grid.set_wrap_indent(true);
            grid.move_to(0, 0);
            for ch in "  01234567XYZ".chars() {
                grid.print(ch);
            }
        }
        app.panes.insert(id, pane);
        app.modes.insert(id, crate::model::mode::Mode::Visual);
        // Anchor at row 0, col 0; cursor at row 1, col 4 ("Z")
        app.selection.visual_anchor = Some((0, 0));
        app.set_nav_cursor(id, (1, 4));
        app.update_visual_selection(id);

        // Soft-wrapped rows should join without newline and skip the hanging indent of 2 spaces
        assert_eq!(app.selected_text().as_deref(), Some("  01234567XYZ"));
    }
}
