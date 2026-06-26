//! Rebuilding the scrollback history at a new width.
//!
//! Word wrap and hanging indent move cells away from where a program wrote
//! them, so they are never applied as output is printed: a program redrawing
//! in place (cursor up N, erase down) would land on the wrong rows. The history
//! is the one place no program can reach, since every cursor motion is bounded
//! by the live screen, so a resize that changes the width rebuilds it there,
//! folding its lines at word boundaries and indenting their continuations.

use std::collections::VecDeque;

use super::reflow::{is_trimmable_blank, LogicalLines};
use super::{CellWidth, Grid, Row, RowRemap};

// ========================================================================
// Data Structures
// ========================================================================

/// How a history rebuild moved the rows it held.
struct HistoryRemap {
    /// For each history row before the rebuild (oldest first), its absolute
    /// row after it.
    new_abs: Vec<usize>,
    /// Absolute row of the oldest history row before the rebuild.
    old_top: usize,
    /// How many rows the rebuild added (or, when negative, removed): every
    /// row below the history moves by this much.
    shift: isize,
}

impl HistoryRemap {
    /// The absolute row that content at absolute row `abs` (counted before
    /// the rebuild) sits at after it. Rows older than the history were
    /// already evicted and map to themselves; rows below it move by `shift`.
    fn map(&self, abs: usize) -> usize {
        match abs.checked_sub(self.old_top) {
            None => abs,
            Some(index) => match self.new_abs.get(index) {
                Some(&new) => new,
                None => abs.saturating_add_signed(self.shift),
            },
        }
    }
}

// ========================================================================
// Grid: history reflow
// ========================================================================

impl Grid {
    /// Whether a width change left the history to be rebuilt at the new
    /// width: the owner should call [`Self::rebuild_history`] once the size
    /// has settled.
    pub fn has_stale_history(&self) -> bool {
        self.history_stale
    }
    /// Rebuild a history a width change left stale (see
    /// [`Self::has_stale_history`]) at the current width, and return how it
    /// moved every row from the oldest history row through the live screen.
    /// Every anchor kept in absolute rows must be pushed through the remap,
    /// as after a [`Self::resize`]. `None` when nothing was stale or moved.
    pub fn rebuild_history(&mut self) -> Option<RowRemap> {
        if !std::mem::take(&mut self.history_stale) {
            return None;
        }
        let old_top = self.trimmed_rows;
        let old_end = self.absolute_live_top() + self.rows;
        let history = self.reflow_history()?;
        Some(RowRemap {
            new_abs: (old_top..old_end).map(|abs| history.map(abs)).collect(),
            old_top,
        })
    }
    /// Rebuild the history at the current width with word wrap and hanging
    /// indent as configured, and report how its rows moved. `None` when
    /// neither setting is on, so the history keeps the widths it was printed
    /// at, or when there is nothing to rebuild.
    ///
    /// A line that continues into the live screen is left as it is: half of
    /// it is still where the program can reach it.
    fn reflow_history(&mut self) -> Option<HistoryRemap> {
        if !self.word_wrap && !self.wrap_indent {
            return None;
        }
        let (logical, tail) = self.collect_history_lines();
        if logical.lines.is_empty() {
            return None;
        }

        let mut fold = Grid::new(self.cols, self.rows);
        fold.fold_on_print = true;
        fold.max_scrollback = usize::MAX;
        fold.word_wrap = self.word_wrap;
        fold.wrap_indent = self.wrap_indent;
        let line_new_abs = fold.replay_logical_lines(&logical);
        let folded = fold.live_row_remap(&logical, &line_new_abs, 0, tail)?;

        let mut rebuilt: VecDeque<Row> = std::mem::take(&mut fold.scrollback);
        for row in 0..=fold.cursor.row {
            let start = row * fold.cols;
            rebuilt.push_back(Row {
                cells: fold.cells[start..start + fold.cols].to_vec(),
                meta: fold.row_meta[row],
            });
        }
        let folded_len = rebuilt.len();
        let old_len = self.scrollback.len();
        rebuilt.extend(self.scrollback.drain(tail..));

        let old_top = self.trimmed_rows;
        let mut new_abs: Vec<usize> = (0..tail).map(|row| old_top + folded.map(row)).collect();
        new_abs.extend((tail..old_len).map(|row| old_top + folded_len + (row - tail)));
        let shift = rebuilt.len() as isize - old_len as isize;

        // The fold keeps every line, only in more or fewer rows; the row
        // ceiling may still evict from the front. Absolute rows keep counting
        // from the first line ever written, so that moves nothing that
        // survives.
        self.scrollback = rebuilt;
        self.history_lines = self
            .scrollback
            .iter()
            .filter(|row| !row.meta.wrapped)
            .count();
        self.trim_history();
        Some(HistoryRemap {
            new_abs,
            old_top,
            shift,
        })
    }
    /// The history's logical lines, each joined from its soft-wrapped rows
    /// with continuation indents and word-break residue left out, and the
    /// index of the first row not among them: the start of a line still
    /// continuing into the live screen, or the history's length.
    fn collect_history_lines(&self) -> (LogicalLines, usize) {
        let tail = match self.scrollback.back() {
            Some(last) if last.meta.wrapped => {
                let mut start = self.scrollback.len() - 1;
                while start > 0 && self.scrollback[start - 1].meta.wrapped {
                    start -= 1;
                }
                start
            }
            _ => self.scrollback.len(),
        };
        let mut logical = LogicalLines {
            cursor: None,
            line_starts: Vec::new(),
            lines: Vec::new(),
        };
        let mut line = Vec::new();
        let mut line_start = 0;
        for (index, row) in self.scrollback.iter().take(tail).enumerate() {
            let is_cont = index > 0 && self.scrollback[index - 1].meta.wrapped;
            let indent = if is_cont { row.meta.indent } else { 0 };
            let len = row.cells.len();
            let row_end = row.meta.break_col.map_or(len, |k| (k + 1).min(len));
            line.extend(
                row.cells[indent.min(row_end)..row_end]
                    .iter()
                    .filter(|cell| cell.width != CellWidth::Spacer)
                    .cloned(),
            );
            if !row.meta.wrapped {
                while line.last().is_some_and(is_trimmable_blank) {
                    line.pop();
                }
                logical.lines.push(std::mem::take(&mut line));
                logical.line_starts.push(line_start);
                line_start = index + 1;
            }
        }
        (logical, tail)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::scroll::HISTORY_ROW_CEILING;
    use super::super::test_support::*;
    use super::*;

    fn print_str(grid: &mut Grid, text: &str) {
        for ch in text.chars() {
            grid.print(ch);
        }
    }

    /// A grid with both history settings on, `text` printed as its own lines
    /// and then scrolled entirely into the history by `rows` blank lines.
    fn grid_with_history(cols: usize, rows: usize, lines: &[&str]) -> Grid {
        let mut grid = Grid::new(cols, rows)
            .with_word_wrap(true)
            .with_wrap_indent(true);
        for line in lines {
            print_str(&mut grid, line);
            grid.carriage_return();
            grid.line_feed();
        }
        for _ in 0..rows {
            grid.line_feed();
        }
        grid
    }

    /// The history's rows as trimmed text, oldest first.
    fn history_text(grid: &Grid) -> Vec<String> {
        grid.scrollback
            .iter()
            .map(|row| {
                let text: String = row.cells.iter().map(|cell| cell.ch).collect();
                text.trim_end().to_string()
            })
            .collect()
    }

    #[test]
    fn test_output_is_never_folded_as_it_prints() {
        // Regression: folding at print time moved cells a program wrote, so
        // an in-place redraw (cursor up, erase down) left a stale row behind.
        let mut grid = Grid::new(10, 4).with_word_wrap(true).with_wrap_indent(true);
        print_str(&mut grid, "abcdef ghijklm nopqrs tuvwxy");
        assert_eq!(
            grid.cursor(),
            (2, 8),
            "28 cells wrap at the margin into 3 rows"
        );
        assert_eq!(row_text(&grid, 0), "abcdef ghi");
    }

    #[test]
    fn test_a_width_change_folds_the_history_at_word_boundaries() {
        let mut grid = grid_with_history(20, 2, &["abcdef ghijklm nopqrs"]);
        grid.resize(10, 2);
        assert!(
            grid.has_stale_history(),
            "a width change leaves it to rebuild"
        );
        grid.rebuild_history();
        let history = history_text(&grid);
        assert_eq!(history[..3], ["abcdef", "ghijklm", "nopqrs"]);
    }

    #[test]
    fn test_a_width_change_indents_history_continuations() {
        let mut grid = grid_with_history(20, 2, &["  abcdefgh ijklmnop"]);
        grid.resize(12, 2);
        grid.rebuild_history();
        let history = history_text(&grid);
        assert_eq!(history[..2], ["  abcdefgh", "  ijklmnop"]);
        assert_eq!(grid.absolute_row_wrap_indent(grid.trimmed_rows + 1), 2);
    }

    #[test]
    fn test_history_is_left_alone_with_both_settings_off() {
        let mut grid = Grid::new(20, 2);
        print_str(&mut grid, "abcdef ghijklm nopq");
        for _ in 0..3 {
            grid.line_feed();
        }
        grid.resize(10, 2);
        assert!(!grid.has_stale_history());
        assert!(grid.rebuild_history().is_none());
        assert_eq!(
            history_text(&grid)[0],
            "abcdef ghijklm nopq",
            "kept at its old width"
        );
    }

    #[test]
    fn test_a_line_still_continuing_on_screen_is_not_folded() {
        // Half of the line is still on the live screen, where the program
        // may redraw it; folding the other half would split it apart.
        let mut grid = Grid::new(4, 2).with_word_wrap(true).with_wrap_indent(true);
        print_str(&mut grid, "ab cd ef gh");
        let before = history_text(&grid);
        grid.history_stale = true;
        let remap = grid.rebuild_history();
        assert!(
            remap.is_none(),
            "nothing but the straddling line to rebuild"
        );
        assert_eq!(history_text(&grid), before);
    }

    #[test]
    fn test_anchors_follow_rows_the_history_rebuild_moved() {
        let mut grid = grid_with_history(20, 2, &["abcdef ghijklm nopq", "tail"]);
        let tail_abs = grid.trimmed_rows + 1;
        grid.resize(10, 2);
        grid.take_row_remap();
        let live_top = grid.absolute_live_top();
        let remap = grid.rebuild_history().expect("the rebuild moved rows");
        let tail_row = remap.map(tail_abs) - grid.trimmed_rows;
        assert_eq!(
            history_text(&grid)[tail_row],
            "tail",
            "the line below the folded one"
        );
        assert_eq!(
            remap.map(live_top),
            grid.absolute_live_top(),
            "the live screen moves down by the rows the fold added"
        );
    }

    #[test]
    fn test_a_narrower_fold_keeps_every_line_under_the_cap() {
        // Regression: the cap counted rows, so folding a line into more rows
        // at a narrower width pushed the oldest lines out of the history.
        let mut grid = grid_with_history(20, 1, &["abcdef ghijklm nopq", "tail"]);
        grid.max_scrollback = 3;
        grid.trim_history();
        assert_eq!(
            grid.history_lines, 3,
            "two lines and the blank one after them"
        );
        grid.resize(10, 1);
        grid.rebuild_history();
        let history = history_text(&grid);
        assert_eq!(history[..4], ["abcdef", "ghijklm", "nopq", "tail"]);
        assert_eq!(grid.history_lines, 3, "folding changed rows, not lines");
    }

    #[test]
    fn test_the_line_cap_evicts_a_line_with_all_its_rows() {
        let mut grid = Grid::new(4, 1).with_max_scrollback(1);
        print_str(&mut grid, "aaaabbbbccccdddd");
        grid.carriage_return();
        grid.line_feed();
        print_str(&mut grid, "e");
        grid.carriage_return();
        grid.line_feed();
        assert_eq!(
            history_text(&grid),
            ["e"],
            "the 4-row line went whole, not cut"
        );
        assert_eq!(grid.history_lines, 1);
    }

    #[test]
    fn test_the_row_ceiling_bounds_one_endless_line() {
        let mut grid = Grid::new(4, 1).with_max_scrollback(1);
        print_str(&mut grid, &"x".repeat(4 * 10));
        assert!(
            grid.scrollback_len() <= HISTORY_ROW_CEILING,
            "a line that never ends must not grow the history without bound"
        );
    }
}
