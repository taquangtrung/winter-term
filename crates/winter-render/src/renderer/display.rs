//! What a cell shows once pane-wide state is applied: an overlay drawn over
//! the grid (an input method's uncommitted text), the screen's reverse video
//! (DECSCNM), concealed text (SGR 8), and blinking text (SGR 5) in its hidden
//! phase. Every pass that reads cells to draw them goes through
//! [`shown_cell`], so all of it is applied in one place and the passes keep
//! reading an ordinary [`Cell`].

use std::borrow::Cow;

use unicode_width::UnicodeWidthChar;

use super::PaneView;
use crate::grid::{Cell, CellWidth, Grid, Style};

// ========================================================================
// Constants
// ========================================================================

/// Columns a double-width character covers.
const WIDE_COLUMNS: usize = 2;

// ========================================================================
// Data Structures
// ========================================================================

/// A run of cells drawn over a pane's grid, in place of what the grid holds
/// there: an input method's preedit text at the cursor, say. Viewport rows.
#[derive(Clone, Debug, PartialEq)]
pub struct CellOverlay {
    cells: Vec<Cell>,
    col: usize,
    row: usize,
}

/// The pane-wide state that changes how its cells are shown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ScreenPresentation {
    /// Whether blinking text is in its shown phase.
    pub(super) blink_visible: bool,
    /// Whether the program put the whole screen in reverse video.
    pub(super) reverse_video: bool,
}

// ========================================================================
// CellOverlay
// ========================================================================

impl CellOverlay {
    /// `text` laid out from viewport cell `(row, col)` in `style`, one cell
    /// per character and two for a double-width one. Zero-width characters
    /// have no cell of their own and are dropped.
    pub fn from_text(row: usize, col: usize, text: &str, style: Style) -> Self {
        let mut cells = Vec::new();
        for ch in text.chars() {
            match UnicodeWidthChar::width(ch) {
                Some(0) => {}
                Some(WIDE_COLUMNS) => {
                    cells.push(Cell {
                        ch,
                        style,
                        width: CellWidth::Wide,
                        ..Cell::default()
                    });
                    cells.push(Cell {
                        style,
                        width: CellWidth::Spacer,
                        ..Cell::default()
                    });
                }
                _ => cells.push(Cell {
                    ch,
                    style,
                    ..Cell::default()
                }),
            }
        }
        Self { cells, col, row }
    }

    /// How many columns the overlay covers.
    pub fn width(&self) -> usize {
        self.cells.len()
    }

    fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        if row != self.row {
            return None;
        }
        self.cells.get(col.checked_sub(self.col)?)
    }
}

// ========================================================================
// PaneView
// ========================================================================

impl PaneView<'_> {
    /// The pane-wide state its cells are shown under.
    pub(super) fn presentation(&self) -> ScreenPresentation {
        ScreenPresentation {
            blink_visible: self.blink_visible,
            reverse_video: self.grid.reverse_video(),
        }
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// The cell drawn at viewport `(row, col)`: the overlay's where it covers
/// the grid, else the grid's own, either as [`displayed_cell`] shows it.
pub(super) fn shown_cell<'a>(
    grid: &'a Grid,
    overlay: Option<&'a CellOverlay>,
    row: usize,
    col: usize,
    screen: ScreenPresentation,
) -> Option<Cow<'a, Cell>> {
    let cell = overlay
        .and_then(|overlay| overlay.cell(row, col))
        .or_else(|| grid.visible_cell(row, col))?;
    Some(displayed_cell(cell, screen))
}

/// `cell` as it is drawn under `screen`: a concealed glyph, or a blinking
/// one in its hidden phase, becomes a blank (the cell keeps its colors,
/// width and decorations), and screen-wide reverse video flips the cell's
/// own reverse attribute, so a reversed cell reads normally again just as
/// xterm shows it. Borrowed unchanged in the common case.
pub(super) fn displayed_cell(cell: &Cell, screen: ScreenPresentation) -> Cow<'_, Cell> {
    let hidden = cell.style.concealed || (cell.style.blink && !screen.blink_visible);
    if !hidden && !screen.reverse_video {
        return Cow::Borrowed(cell);
    }
    let mut shown = cell.clone();
    if hidden {
        shown.ch = ' ';
        shown.tail = None;
    }
    shown.style.reversed ^= screen.reverse_video;
    Cow::Owned(shown)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Style;

    const NORMAL: ScreenPresentation = ScreenPresentation {
        blink_visible: true,
        reverse_video: false,
    };

    fn styled(style: Style) -> Cell {
        Cell {
            ch: 'x',
            style,
            ..Cell::default()
        }
    }

    #[test]
    fn test_plain_cells_are_borrowed_untouched() {
        let cell = styled(Style::default());
        assert!(matches!(displayed_cell(&cell, NORMAL), Cow::Borrowed(_)));
    }

    #[test]
    fn test_concealed_and_hidden_blinking_glyphs_draw_as_blanks() {
        let concealed = styled(Style {
            concealed: true,
            ..Style::default()
        });
        assert_eq!(displayed_cell(&concealed, NORMAL).ch, ' ');
        let blinking = styled(Style {
            blink: true,
            ..Style::default()
        });
        assert_eq!(displayed_cell(&blinking, NORMAL).ch, 'x', "shown phase");
        let hidden_phase = ScreenPresentation {
            blink_visible: false,
            ..NORMAL
        };
        assert_eq!(displayed_cell(&blinking, hidden_phase).ch, ' ');
    }

    #[test]
    fn test_screen_reverse_video_flips_each_cells_own_reverse() {
        let reversed_screen = ScreenPresentation {
            reverse_video: true,
            ..NORMAL
        };
        let plain = styled(Style::default());
        assert!(displayed_cell(&plain, reversed_screen).style.reversed);
        let reversed = styled(Style {
            reversed: true,
            ..Style::default()
        });
        assert!(!displayed_cell(&reversed, reversed_screen).style.reversed);
    }

    #[test]
    fn test_an_overlay_lays_out_wide_text_and_covers_only_its_run() {
        let style = Style::default();
        let overlay = CellOverlay::from_text(1, 2, "a\u{4e2d}\u{301}b", style);
        assert_eq!(overlay.width(), 4, "a, the wide glyph and its spacer, b");
        assert_eq!(overlay.cell(1, 3).map(|c| c.width), Some(CellWidth::Wide));
        assert_eq!(overlay.cell(1, 4).map(|c| c.width), Some(CellWidth::Spacer));
        assert_eq!(overlay.cell(1, 1), None, "left of the run");
        assert_eq!(overlay.cell(0, 2), None, "another row");
        let mut grid = Grid::new(10, 3);
        grid.print('z');
        let shown = shown_cell(&grid, Some(&overlay), 1, 2, NORMAL).expect("cell");
        assert_eq!(shown.ch, 'a');
        let under = shown_cell(&grid, Some(&overlay), 0, 0, NORMAL).expect("cell");
        assert_eq!(under.ch, 'z', "the grid shows where the overlay is not");
    }
}
