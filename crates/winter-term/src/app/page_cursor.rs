//! A Vim-style text cursor over a tool page, for selecting and copying rows.
//!
//! A page binds most of the Vim vocabulary for its own navigation: `j` and `k`
//! step entries, `g` and `G` jump, `/` searches. A cursor for selecting *text*
//! therefore cannot share those keys, so it is a mode of its own that `v`
//! enters and `Esc` leaves. While it is up it takes every key before the page
//! sees one, which is what frees `V`, `y` and the motions to mean here what
//! they mean in Vim, whatever the page binds them to.
//!
//! The rows it addresses are the page's own painted grid, not the terminal
//! hidden underneath: see [`App::selection_grid`].

use crate::model::input::{Key, KeyCode};
use crate::model::layout::PaneId;

use super::{App, Selection};

// ========================================================================
// Data Structures
// ========================================================================

/// A text cursor over one page, and the selection it is extending.
pub(crate) struct PageCursor {
    /// Where a visual selection was started from, or `None` when the cursor is
    /// only being moved.
    anchor: Option<(usize, usize)>,
    pub(crate) col: usize,
    /// Whether the selection covers whole lines rather than characters.
    linewise: bool,
    /// The pane whose page this cursor is over.
    pub(crate) pane: PaneId,
    pub(crate) row: usize,
}

// ========================================================================
// App: the page text cursor
// ========================================================================

impl App {
    /// Start a text cursor over `pane`'s page, parked on the row the page has
    /// its own cursor line on, so selecting begins where the user was looking.
    ///
    /// It lands on that row's first non-blank column, the way `^` does in Vim.
    /// Column zero is a blank gutter on every page that draws one (Dir's mark
    /// column, Git's status column), so starting there would put the cursor on
    /// an empty cell against the left edge.
    ///
    /// The selection is anchored straight away, because the cursor was already
    /// on screen before `v` was pressed: a page pane draws one on its active
    /// row at all times. `v` therefore means what it means in Vim, "start
    /// selecting here", rather than "produce a cursor", and a motion after it
    /// extends a selection instead of silently selecting nothing.
    pub(crate) fn start_page_cursor(&mut self, pane: PaneId) {
        let row = self
            .pages
            .get(&pane)
            .and_then(|slot| slot.cursor_line)
            .unwrap_or(0);
        let col = self
            .selection_grid(pane)
            .map(|grid| first_non_blank(grid, row))
            .unwrap_or(0);
        self.page_cursor = Some(PageCursor {
            anchor: Some((row, col)),
            col,
            linewise: false,
            pane,
            row,
        });
        self.sync_page_selection();
        self.dirty = true;
    }

    /// Drop the text cursor and any selection it was building.
    pub(crate) fn stop_page_cursor(&mut self) {
        if let Some(cursor) = self.page_cursor.take() {
            self.drop_selection_in_pane(cursor.pane);
        }
        self.dirty = true;
    }

    /// Route one key into the page text cursor. Returns whether it was used.
    ///
    /// Every key is answered while the cursor is up, including the ones the
    /// page binds: a mode that let some keys through to the listing underneath
    /// would move the selection and the listing at the same time.
    pub(crate) fn handle_page_cursor_key(&mut self, key: &Key) -> bool {
        // Chorded keys stay with the window: splitting, zooming and moving
        // focus have to work from here, exactly as they do from a page, and
        // none of them collide with a motion.
        if key.alt || key.ctrl {
            return false;
        }
        let Some(cursor) = self.page_cursor.as_ref() else {
            return false;
        };
        let pane = cursor.pane;
        // Focus moved on while the cursor was up, so it belongs to a pane the
        // keys are no longer going to.
        if pane != self.tab().focused() {
            self.stop_page_cursor();
            return false;
        }
        let Some(grid) = self.selection_grid(pane) else {
            self.stop_page_cursor();
            return true;
        };
        let last_row = grid.rows().saturating_sub(1);
        let last_col = grid.cols().saturating_sub(1);
        let (mut row, mut col) = (cursor.row, cursor.col);
        let (mut anchor, mut linewise) = (cursor.anchor, cursor.linewise);

        match key.code {
            KeyCode::Escape => {
                self.stop_page_cursor();
                return true;
            }
            // Copying is the point of the cursor, so it ends here: leaving the
            // mode up after a yank would swallow the next key the user meant
            // for the listing.
            KeyCode::Char('y') | KeyCode::Enter => {
                self.copy_selection();
                self.copy_selection_to_primary();
                self.page_cursor = None;
                self.dirty = true;
                return true;
            }
            KeyCode::Char('v') => {
                anchor = match anchor {
                    Some(_) if !linewise => None,
                    _ => Some((row, col)),
                };
                linewise = false;
            }
            KeyCode::Char('V') => {
                anchor = match anchor {
                    Some(_) if linewise => None,
                    _ => Some((row, col)),
                };
                linewise = true;
            }
            KeyCode::Char('h') | KeyCode::Left => col = col.saturating_sub(1),
            KeyCode::Char('l') | KeyCode::Right => col = (col + 1).min(last_col),
            KeyCode::Char('j') | KeyCode::Down => row = (row + 1).min(last_row),
            KeyCode::Char('k') | KeyCode::Up => row = row.saturating_sub(1),
            KeyCode::Char('0') | KeyCode::Home => col = 0,
            KeyCode::Char('$') | KeyCode::End => col = last_col,
            KeyCode::Char('g') => row = 0,
            KeyCode::Char('G') => row = last_row,
            KeyCode::Char('w') => col = next_word(grid, row, col, last_col, true),
            KeyCode::Char('b') => col = next_word(grid, row, col, last_col, false),
            _ => return true,
        }

        let Some(cursor) = self.page_cursor.as_mut() else {
            return true;
        };
        cursor.anchor = anchor;
        cursor.col = col;
        cursor.linewise = linewise;
        cursor.row = row;
        self.sync_page_selection();
        self.dirty = true;
        true
    }

    /// Rebuild the highlighted span from the cursor and its anchor.
    ///
    /// A page's grid keeps no scrollback, so its viewport rows are already the
    /// absolute rows a [`Selection`] names and no conversion is needed.
    fn sync_page_selection(&mut self) {
        let Some(cursor) = self.page_cursor.as_ref() else {
            return;
        };
        let Some((anchor_row, anchor_col)) = cursor.anchor else {
            let pane = cursor.pane;
            self.drop_selection_in_pane(pane);
            return;
        };
        let last_col = self
            .selection_grid(cursor.pane)
            .map(|grid| grid.cols().saturating_sub(1))
            .unwrap_or(0);
        let (start_col, end_col) = if cursor.linewise {
            (0, last_col)
        } else {
            (anchor_col, cursor.col)
        };
        self.selection.span = Some(Selection {
            block: false,
            end_col,
            end_row: cursor.row,
            pane: cursor.pane,
            start_col,
            start_row: anchor_row,
        });
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// The first column of `row` holding something other than a space, or zero
/// when the row is blank.
pub(crate) fn first_non_blank(grid: &winter_render::Grid, row: usize) -> usize {
    (0..grid.cols())
        .find(|col| {
            grid.cell(row, *col)
                .is_some_and(|cell| !cell.ch.is_whitespace())
        })
        .unwrap_or(0)
}

/// The column one word away from `col` on `row`, in the given direction.
fn next_word(
    grid: &winter_render::Grid,
    row: usize,
    col: usize,
    last_col: usize,
    forward: bool,
) -> usize {
    let blank = |c: usize| {
        grid.cell(row, c)
            .map(|cell| cell.ch.is_whitespace())
            .unwrap_or(true)
    };
    let mut at = col;
    if forward {
        while at < last_col && !blank(at) {
            at += 1;
        }
        while at < last_col && blank(at) {
            at += 1;
        }
    } else {
        while at > 0 && blank(at.saturating_sub(1)) {
            at -= 1;
        }
        while at > 0 && !blank(at.saturating_sub(1)) {
            at -= 1;
        }
    }
    at
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::input::WindowKeymap;
    use crate::tools::keys::KeysPage;
    use winter_render::Grid;

    /// An app whose focused pane is covered by a page already painted with
    /// `lines`, which is what a real frame would have left on the slot.
    fn app_with_painted_page(lines: &[&str]) -> (App, PaneId) {
        let mut app = App::new();
        // The default tab lays out a single pane, `PaneId(0)`.
        let pane = PaneId(0);
        app.show_page("keys", Box::new(KeysPage::new(&WindowKeymap::default())));
        let mut grid = Grid::new(40, lines.len().max(1));
        for (row, line) in lines.iter().enumerate() {
            grid.move_to(row, 0);
            for ch in line.chars() {
                grid.print(ch);
            }
        }
        if let Some(slot) = app.pages.get_mut(&pane) {
            slot.painted = Some(grid);
        }
        (app, pane)
    }

    fn press(app: &mut App, code: KeyCode) -> bool {
        app.handle_page_cursor_key(&Key {
            alt: false,
            code,
            ctrl: false,
            shift: false,
        })
    }

    #[test]
    fn test_a_visual_selection_copies_the_page_rows_not_the_terminal_underneath() {
        // The whole point of the cursor: a page covers a pane whose shell is
        // still running, and a selection resolved against the pane's own grid
        // names the output hidden behind the listing.
        let (mut app, pane) = app_with_painted_page(&["alpha", "bravo", "charlie"]);
        app.start_page_cursor(pane);
        assert!(
            press(&mut app, KeyCode::Char('V')),
            "V starts a linewise span"
        );
        assert!(press(&mut app, KeyCode::Char('j')));
        assert_eq!(app.selected_text().as_deref(), Some("alpha\nbravo"));
    }

    #[test]
    fn test_the_cursor_starts_on_the_first_non_blank_column() {
        // Every page draws a blank gutter in column zero (Dir's mark column,
        // Git's status column). Starting there put a block cursor on an empty
        // cell hard against the left edge, which reads as a margin stripe
        // rather than a cursor, and was reported as there being no cursor.
        let (mut app, pane) = app_with_painted_page(&["   indented", "x"]);
        app.start_page_cursor(pane);
        let cursor = app.page_cursor.as_ref().expect("a cursor");
        assert_eq!(cursor.col, 3, "lands on the `i`, not the blank gutter");
    }

    #[test]
    fn test_a_blank_row_leaves_the_cursor_at_column_zero() {
        // Nothing to land on, and scanning off the end of the row would be
        // worse than the gutter it was avoiding.
        let (mut app, pane) = app_with_painted_page(&["   "]);
        app.start_page_cursor(pane);
        assert_eq!(app.page_cursor.as_ref().expect("a cursor").col, 0);
    }

    #[test]
    fn test_v_anchors_straight_away_so_the_next_motion_selects() {
        // Regression: `v` used to only produce the cursor, leaving the anchor
        // unset, so `v l l y` copied nothing at all. The cursor is on screen
        // before `v` is pressed now, so `v` has to mean "start selecting".
        let (mut app, pane) = app_with_painted_page(&["alpha", "bravo"]);
        app.start_page_cursor(pane);
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.selected_text().as_deref(), Some("alp"));
    }

    #[test]
    fn test_v_a_second_time_drops_the_selection() {
        // The toggle Vim has: `v` out of charwise Visual leaves the cursor
        // where it is with nothing selected.
        let (mut app, pane) = app_with_painted_page(&["alpha", "bravo"]);
        app.start_page_cursor(pane);
        press(&mut app, KeyCode::Char('l'));
        assert!(
            app.selection.span.is_some(),
            "fixture: something is selected"
        );
        press(&mut app, KeyCode::Char('v'));
        assert!(app.selection.span.is_none());
    }

    #[test]
    fn test_escape_drops_the_cursor_and_its_selection() {
        let (mut app, pane) = app_with_painted_page(&["alpha", "bravo"]);
        app.start_page_cursor(pane);
        press(&mut app, KeyCode::Char('l'));
        assert!(
            app.selection.span.is_some(),
            "fixture: something is selected"
        );
        press(&mut app, KeyCode::Escape);
        assert!(app.page_cursor.is_none());
        assert!(app.selection.span.is_none(), "the highlight goes with it");
    }

    #[test]
    fn test_chorded_keys_fall_through_to_the_window() {
        // Regression risk: a mode that answers every key traps the user in it,
        // with no way to split, zoom, or move focus until they press Escape.
        let (mut app, pane) = app_with_painted_page(&["alpha"]);
        app.start_page_cursor(pane);
        let chord = Key {
            alt: true,
            code: KeyCode::Char('h'),
            ctrl: false,
            shift: false,
        };
        assert!(!app.handle_page_cursor_key(&chord));
        assert!(
            app.page_cursor.is_some(),
            "and the cursor survives the chord"
        );
    }

    #[test]
    fn test_closing_the_page_takes_the_cursor_with_it() {
        // A cursor left behind would keep answering keys for rows nothing is
        // painting any more.
        let (mut app, pane) = app_with_painted_page(&["alpha"]);
        app.start_page_cursor(pane);
        app.close_page(pane);
        assert!(app.page_cursor.is_none());
    }
}
