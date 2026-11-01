//! Indentation guides: a faint line at each level of a line's indentation,
//! in the space the indentation leaves. A level is as wide as the file
//! indents by around the line: a tab where it indents with tabs, else the
//! narrowest run of leading spaces nearby. A blank line takes the guides of
//! the shallower of the lines either side of it, so a block's guides run on
//! through the blank lines inside it.

use winter_model::paint::{Style, TAB_WIDTH};

use super::Cell;

// ========================================================================
// Constants
// ========================================================================

/// How many lines either side a line's indentation is read from: enough to
/// see past a long block, few enough to stay cheap for every row painted.
const REACH: usize = 100;

/// The narrowest and widest a level of indentation is taken to be.
const MIN_STEP: usize = 2;
const MAX_STEP: usize = 8;

/// A level the file gives no clue of, as the editor indents by.
const DEFAULT_STEP: usize = 4;

/// What a guide is drawn with.
const GUIDE: char = '│';

// ========================================================================
// Functions
// ========================================================================

/// The display columns line `index` of `lines` wears a guide at: one at each
/// level of its indentation.
pub(super) fn columns(lines: &[String], index: usize) -> Vec<usize> {
    let depth = depth(lines, index);
    if depth == 0 {
        return Vec::new();
    }
    (0..depth).step_by(step(lines, index)).collect()
}

/// Draw a guide at each of `columns` in a row's `cells`, where the cell is
/// blank, opening a blank line out to reach them.
pub(super) fn mark(cells: &mut Vec<Cell>, columns: &[usize]) {
    let Some(last) = columns.last() else {
        return;
    };
    if cells.len() <= *last {
        cells.resize(last + 1, (' ', Style::Normal, None, false));
    }
    for col in columns {
        if let Some(cell) = cells.get_mut(*col).filter(|cell| cell.0 == ' ') {
            cell.0 = GUIDE;
            cell.1 = Style::IndentGuide;
        }
    }
}

/// How many display columns of line `index` are indentation: a blank line's
/// the shallower of its nearest lines with text above and below.
fn depth(lines: &[String], index: usize) -> usize {
    if let Some(depth) = lines.get(index).and_then(|line| indent_width(line)) {
        return depth;
    }
    let above = (index.saturating_sub(REACH)..index)
        .rev()
        .find_map(|at| indent_width(&lines[at]));
    let below =
        (index + 1..lines.len().min(index + 1 + REACH)).find_map(|at| indent_width(&lines[at]));
    above
        .zip(below)
        .map_or(0, |(above, below)| above.min(below))
}

/// How wide a level of indentation is around line `index`.
fn step(lines: &[String], index: usize) -> usize {
    let near = &lines[index.saturating_sub(REACH)..lines.len().min(index + REACH)];
    if near.iter().any(|line| line.starts_with('\t')) {
        return TAB_WIDTH;
    }
    near.iter()
        .filter_map(|line| indent_width(line))
        .filter(|width| *width >= MIN_STEP)
        .min()
        .map_or(DEFAULT_STEP, |width| width.min(MAX_STEP))
}

/// The display width of `line`'s indentation, a tab as wide as it paints;
/// `None` for a line with no text.
fn indent_width(line: &str) -> Option<usize> {
    let mut width = 0;
    for c in line.chars() {
        match c {
            ' ' => width += 1,
            '\t' => width += TAB_WIDTH,
            c if c.is_whitespace() => width += 1,
            _ => return Some(width),
        }
    }
    None
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn test_a_guide_marks_each_level_of_the_indentation_the_file_uses() {
        let two = lines(&["fn a() {", "  if x {", "    y();", "  }", "}"]);
        assert_eq!(columns(&two, 2), [0, 2], "two-space levels");
        assert_eq!(columns(&two, 0), Vec::<usize>::new(), "none at the margin");
        let tabs = lines(&["a", "\t\tb"]);
        assert_eq!(columns(&tabs, 1), [0, TAB_WIDTH], "a tab is a level");
    }

    #[test]
    fn test_a_blank_line_takes_the_shallower_guides_of_its_neighbors() {
        let block = lines(&["    a", "", "        b", "", "c"]);
        assert_eq!(columns(&block, 1), [0], "between four and eight: four's");
        assert_eq!(
            columns(&block, 3),
            Vec::<usize>::new(),
            "next to the margin: none"
        );
    }

    #[test]
    fn test_guides_go_only_where_the_row_is_blank() {
        let mut cells: Vec<Cell> = "  x"
            .chars()
            .map(|c| (c, Style::Normal, None, false))
            .collect();
        mark(&mut cells, &[0, 2]);
        let drawn: String = cells.iter().map(|cell| cell.0).collect();
        assert_eq!(drawn, "│ x", "the text keeps its cell");
        let mut blank: Vec<Cell> = Vec::new();
        mark(&mut blank, &[0, 4]);
        assert_eq!(blank.iter().map(|cell| cell.0).collect::<String>(), "│   │");
    }
}
