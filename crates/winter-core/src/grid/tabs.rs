//! Horizontal tab stops: where HT (`\t`), CHT (`CSI n I`) and CBT
//! (`CSI n Z`) move the cursor, set by HTS (`ESC H`) and cleared by TBC
//! (`CSI g`). A fresh terminal has one every eight columns, which is what
//! `it#8` in terminfo promises.

use super::Grid;

// ========================================================================
// Constants
// ========================================================================

/// Columns between the stops a fresh terminal starts with.
const DEFAULT_TAB_INTERVAL: usize = 8;
/// TBC parameter: clear the stop at the cursor's column.
const TBC_AT_CURSOR: usize = 0;
/// TBC parameter: clear every stop.
const TBC_ALL: usize = 3;
const CSI: &str = "\x1b[";
/// HTS: set a stop at the cursor's column.
const HTS: &str = "\x1bH";

// ========================================================================
// Data Structures
// ========================================================================

/// Which columns hold a tab stop.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TabStops {
    stops: Vec<bool>,
}

// ========================================================================
// TabStops
// ========================================================================

impl TabStops {
    /// The default stops for a `cols`-wide screen.
    pub(super) fn new(cols: usize) -> Self {
        Self {
            stops: (0..cols).map(is_default_stop).collect(),
        }
    }

    /// Follow a width change: the stops already set keep their columns, and
    /// any new columns get the default stops.
    pub(super) fn resize(&mut self, cols: usize) {
        let old = self.stops.len();
        self.stops.truncate(cols);
        self.stops.extend((old..cols).map(is_default_stop));
    }

    /// The first stop right of `col`, or the last column when none is left.
    fn next(&self, col: usize) -> usize {
        let last = self.stops.len().saturating_sub(1);
        (col + 1..self.stops.len())
            .find(|&c| self.stops[c])
            .unwrap_or(last)
    }

    /// The first stop left of `col`, or column 0 when none is left.
    fn previous(&self, col: usize) -> usize {
        (0..col.min(self.stops.len()))
            .rev()
            .find(|&c| self.stops[c])
            .unwrap_or(0)
    }

    fn set(&mut self, col: usize, on: bool) {
        if let Some(stop) = self.stops.get_mut(col) {
            *stop = on;
        }
    }

    /// The escape sequences that give a fresh terminal of the same width
    /// these stops, or `None` when they are the default.
    pub(super) fn restore_sequence(&self) -> Option<String> {
        if *self == Self::new(self.stops.len()) {
            return None;
        }
        let mut out = format!("{CSI}{TBC_ALL}g");
        for col in (0..self.stops.len()).filter(|&c| self.stops[c]) {
            out.push_str(&format!("{CSI}{}G{HTS}", col + 1));
        }
        Some(out)
    }
}

// ========================================================================
// Grid: tab stops
// ========================================================================

impl Grid {
    /// Advance the cursor to the next tab stop (HT).
    pub fn tab(&mut self) {
        self.tab_forward(1);
    }

    /// Advance the cursor `n` tab stops, stopping at the last column (CHT).
    pub fn tab_forward(&mut self, n: usize) {
        self.cursor.wrap_pending = false;
        for _ in 0..n.min(self.cols) {
            self.cursor.col = self.tabs.next(self.cursor.col);
        }
    }

    /// Move the cursor back `n` tab stops, stopping at column 0 (CBT).
    /// Renderers that minimize cursor-motion bytes (agy's, for one) use it to
    /// step from a just-redrawn spinner back to the input line; dropping it
    /// leaves the cursor stranded at the end of the spinner text.
    pub fn tab_backward(&mut self, n: usize) {
        self.cursor.wrap_pending = false;
        for _ in 0..n.min(self.cols) {
            self.cursor.col = self.tabs.previous(self.cursor.col);
        }
    }

    /// Set a tab stop at the cursor's column (HTS).
    pub fn set_tab_stop(&mut self) {
        self.tabs.set(self.cursor.col, true);
    }

    /// Clear tab stops (TBC): `0` clears the one at the cursor's column, `3`
    /// clears them all, and any other parameter is ignored.
    pub fn clear_tab_stops(&mut self, which: usize) {
        match which {
            TBC_AT_CURSOR => self.tabs.set(self.cursor.col, false),
            TBC_ALL => self.tabs.stops.fill(false),
            _ => {}
        }
    }
}

// ========================================================================
// Free functions
// ========================================================================

fn is_default_stop(col: usize) -> bool {
    col > 0 && col.is_multiple_of(DEFAULT_TAB_INTERVAL)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tab_advances_to_the_next_default_stop_and_clamps() {
        let mut grid = Grid::new(20, 1);
        grid.print('x');
        grid.tab();
        assert_eq!(grid.cursor(), (0, 8));
        grid.tab_forward(5);
        assert_eq!(grid.cursor(), (0, 19), "no stop left: the last column");
    }

    #[test]
    fn test_backtab_from_on_and_between_stops() {
        let mut grid = Grid::new(40, 1);
        grid.move_to_column(16);
        grid.tab_backward(1);
        assert_eq!(grid.cursor(), (0, 8), "on a stop: back to the one before");
        grid.move_to_column(11);
        grid.tab_backward(9);
        assert_eq!(grid.cursor(), (0, 0));
    }

    #[test]
    fn test_custom_stops_replace_the_defaults() {
        let mut grid = Grid::new(20, 1);
        grid.clear_tab_stops(TBC_ALL);
        grid.move_to_column(3);
        grid.set_tab_stop();
        grid.move_to_column(0);
        grid.tab();
        assert_eq!(grid.cursor(), (0, 3));
        grid.tab();
        assert_eq!(grid.cursor(), (0, 19), "8 and 16 were cleared");
        grid.tab_backward(1);
        assert_eq!(grid.cursor(), (0, 3));
    }

    #[test]
    fn test_clearing_the_stop_at_the_cursor_keeps_the_others() {
        let mut grid = Grid::new(20, 1);
        grid.move_to_column(8);
        grid.clear_tab_stops(TBC_AT_CURSOR);
        grid.move_to_column(0);
        grid.tab();
        assert_eq!(grid.cursor(), (0, 16));
    }

    #[test]
    fn test_resize_keeps_set_stops_and_defaults_the_new_columns() {
        let mut stops = TabStops::new(10);
        stops.set(3, true);
        stops.resize(20);
        assert!(stops.stops[3] && stops.stops[8] && stops.stops[16]);
        stops.resize(5);
        assert_eq!(stops.stops.len(), 5);
    }

    #[test]
    fn test_restore_sequence_is_absent_for_defaults_and_rebuilds_custom_stops() {
        assert_eq!(TabStops::new(20).restore_sequence(), None);
        let mut stops = TabStops::new(20);
        stops.set(8, false);
        stops.set(3, true);
        assert_eq!(
            stops.restore_sequence().as_deref(),
            Some("\x1b[3g\x1b[4G\x1bH\x1b[17G\x1bH")
        );
    }
}
