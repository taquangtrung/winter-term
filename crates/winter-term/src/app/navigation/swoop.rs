//! Buffer Swoop: fuzzy line search over the active pane's scrollback and grid.
//!
//! Extracts non-blank lines from the full scrollback and viewport into
//! line-indexed palette entries, supporting real-time live preview navigation.

use winter_render::Grid;

// ========================================================================
// Extraction
// ========================================================================

/// Extract all non-blank lines across `grid`'s full scrollback and viewport as
/// `(abs_row, text)` pairs, with trailing whitespace trimmed per line.
pub(crate) fn extract_swoop_lines(grid: &Grid) -> Vec<(usize, String)> {
    let total_rows = grid.scrollback_len() + grid.rows();
    let mut lines = Vec::new();

    for abs_row in 0..total_rows {
        let mut line = String::with_capacity(grid.cols());
        for col in 0..grid.cols() {
            if let Some(cell) = grid.absolute_cell(abs_row, col) {
                if cell.ch != '\0' {
                    line.push(cell.ch);
                }
            }
        }
        while line.ends_with(|c: char| c.is_whitespace()) {
            line.pop();
        }
        if !line.trim().is_empty() {
            lines.push((abs_row, line));
        }
    }

    lines
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_swoop_lines_skips_blank_rows_and_trims_trailing_whitespace() {
        let mut grid = Grid::new(20, 3);
        grid.move_to(0, 0);
        for ch in "first line   ".chars() {
            grid.print(ch);
        }
        // row 1 is left blank
        grid.move_to(2, 0);
        for ch in "third line".chars() {
            grid.print(ch);
        }

        let lines = extract_swoop_lines(&grid);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], (0, "first line".to_string()));
        assert_eq!(lines[1], (2, "third line".to_string()));
    }
}
