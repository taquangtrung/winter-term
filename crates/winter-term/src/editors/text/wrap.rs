//! Lines folded to the pane, and moving over the folds. With wrapping on,
//! each line is cut into screen rows at the last word that fits, by the
//! cells its characters take, and the editor paints one row per screen row,
//! so the caret, a click, and the cursor's band all fall where the text is.
//! The rows a line continues on are indented as `editor-wrap-indent` says:
//! not at all, under the line's indentation, or under its text past what
//! opens it (a list item's bullet, a heading's `#`s, a comment's marker).
//!
//! The motions then go by screen rows, as Vim's `gj` and Emacs'
//! `visual-line-mode` do: in Normal and Visual mode `j`, `k`, the arrows,
//! `H`, `M`, `L`, and the paging keys, with `g0`, `g^`, `g$`, and `gm` on the
//! screen row and `gj` and `gk` left for whole lines; while typing `C-n`,
//! `C-p`, the arrows, `C-a`, `C-e`, `Home`, `End`, `C-v`, and `M-v`. An
//! operator (`dj`, `d$`) still takes whole lines, as it does in Vim.

use crate::model::editor::EditorOutcome;
use crate::model::input::{Key, KeyCode, Mods};
use crate::model::paint::WrapIndent;
use crate::model::textedit::modal::{EditMode, Pending};

use super::comment::{self, Marker};
use super::writing::Prose;
#[cfg(test)]
use super::cell_width;
use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// The least of a row a continuation's indent leaves for text: past it the
/// indent is dropped, so a deeply nested line still reads.
const MIN_TEXT_CELLS: usize = 12;

/// What opens a LaTeX list item.
const LATEX_ITEM: &str = "\\item";

// ========================================================================
// Functions
// ========================================================================

/// Where each screen row of `line` starts, as a character column, folded to
/// `cols` cells, the rows after the first `hang` cells narrower: at the
/// start of the last word that fits, or at the margin where a word is wider
/// than the row. Whitespace at a fold stays at the end of the row it
/// follows, so every character is on exactly one row.
#[cfg(test)]
pub(super) fn fold_starts(line: &str, cols: usize, hang: usize) -> Vec<usize> {
    let chars: Vec<char> = line.chars().collect();
    let widths: Vec<usize> = chars.iter().map(|c| cell_width(*c)).collect();
    fold_starts_by(&chars, &widths, cols, hang)
}

/// `fold_starts` for the characters `chars`, each taking the cells
/// `widths` says: none for one whose markup marker hides.
pub(super) fn fold_starts_by(chars: &[char], widths: &[usize], cols: usize, hang: usize) -> Vec<usize> {
    let mut starts = vec![0];
    if cols == 0 {
        return starts;
    }
    let mut pos = 0;
    while pos < chars.len() {
        let cols = match pos {
            0 => cols,
            _ => cols.saturating_sub(hang).max(1),
        };
        let mut used = 0;
        let mut end = pos;
        while end < chars.len() && used + widths[end] <= cols {
            used += widths[end];
            end += 1;
        }
        if end == chars.len() {
            break;
        }
        // A character wider than the whole row still takes a row of its own.
        if end == pos {
            end = pos + 1;
        }
        let next = match chars[end].is_whitespace() {
            // The overflow starts in whitespace: the row is full words, and
            // the next starts at the word after the spaces.
            true => (end..chars.len()).find(|at| !chars[*at].is_whitespace()).unwrap_or(chars.len()),
            false => (pos + 1..=end)
                .rev()
                .find(|at| chars[at - 1].is_whitespace() && !chars[*at].is_whitespace())
                .unwrap_or(end),
        };
        if next >= chars.len() {
            break;
        }
        starts.push(next);
        pos = next;
    }
    starts
}

/// How many characters open `line` before its text: a Markdown list item's
/// bullet or number and task box, a heading's `#`s, a quote's `>`s, a LaTeX
/// `\item`, or a line comment's marker (`comment`), each with the spaces
/// after it; else its indentation.
fn semantic_lead(line: &str, prose: Option<Prose>, comment: Option<&str>) -> usize {
    let chars: Vec<char> = line.chars().collect();
    let indent = chars.iter().take_while(|c| c.is_whitespace()).count();
    let rest: String = chars[indent..].iter().collect();
    let spaces_after = |at: usize| at + chars[at.min(chars.len())..].iter().take_while(|c| **c == ' ' || **c == '\t').count();
    let opened = match prose {
        Some(Prose::Markdown) => markdown_marker(&rest),
        Some(Prose::Latex) => rest.starts_with(LATEX_ITEM).then(|| {
            let after = &rest[LATEX_ITEM.len()..];
            // An item's own label, `\item[label]`, is part of what opens it.
            match after.strip_prefix('[').and_then(|label| label.find(']')) {
                Some(close) => LATEX_ITEM.len() + close + 2,
                None => LATEX_ITEM.len(),
            }
        }),
        None => None,
    };
    let opened = opened.or_else(|| {
        let marker = comment?;
        rest.starts_with(marker).then(|| {
            // The whole run of the marker (`///`, `##`), as it is written.
            rest.chars().take_while(|c| marker.contains(*c)).count()
        })
    });
    match opened {
        Some(length) if length > 0 => {
            let end = spaces_after(indent + length);
            // A marker with no space after it is the text itself (`#tag`);
            // one whose run takes its spaces in (`- [ ] `, `> > `) has them.
            let spaced = end > indent + length || chars.get(indent + length - 1).is_some_and(|c| c.is_whitespace());
            match spaced || end == chars.len() {
                true => end,
                false => indent,
            }
        }
        _ => indent,
    }
}

/// How many characters a Markdown list item's bullet or number and task box,
/// a heading's `#`s, or a quote's `>`s take at the start of `rest`.
fn markdown_marker(rest: &str) -> Option<usize> {
    let chars: Vec<char> = rest.chars().collect();
    let first = *chars.first()?;
    let length = match first {
        '-' | '+' | '*' if chars.get(1).is_some_and(|c| *c == ' ') => {
            let task = ["[ ] ", "[x] ", "[X] "].iter().find(|task| rest[2..].starts_with(**task)).map_or(0, |task| task.len());
            1 + task + usize::from(task > 0)
        }
        '#' => chars.iter().take_while(|c| **c == '#').count(),
        '>' => chars.iter().take_while(|c| **c == '>' || **c == ' ').count(),
        c if c.is_ascii_digit() => {
            let digits = chars.iter().take_while(|c| c.is_ascii_digit()).count();
            match chars.get(digits) {
                Some('.' | ')') => digits + 1,
                _ => return None,
            }
        }
        _ => return None,
    };
    Some(length)
}

// ========================================================================
// TextEditor: the folds
// ========================================================================

impl TextEditor {
    /// Where each screen row of line `index` starts, as a character column:
    /// the line's one row where nothing is folded.
    pub(super) fn segments(&self, index: usize) -> Vec<usize> {
        match self.fold {
            Some(cols) => {
                let chars: Vec<char> = self.line_of(index).chars().collect();
                fold_starts_by(&chars, &self.char_widths(index), cols, self.hang_of(index))
            }
            None => vec![0],
        }
    }

    /// How many cells the rows line `index` continues on are indented by,
    /// as `editor-wrap-indent` says, dropped where it would leave too
    /// little of the row for text.
    pub(super) fn hang_of(&self, index: usize) -> usize {
        let Some(cols) = self.fold else {
            return 0;
        };
        let line = self.line_of(index);
        let lead = match self.wrap_indent {
            WrapIndent::None => 0,
            WrapIndent::Same => line.chars().take_while(|c| c.is_whitespace()).count(),
            WrapIndent::Semantic => {
                let marker = self.doc().grammar.and_then(|grammar| comment::marker_of(grammar.name()));
                let comment = match marker {
                    Some(Marker::Line(opener)) => Some(opener),
                    _ => None,
                };
                semantic_lead(&line, self.prose(), comment)
            }
        };
        let hang = self.cells_between(index, 0, lead);
        match hang + MIN_TEXT_CELLS <= cols {
            true => hang,
            false => 0,
        }
    }

    /// How many cells screen row `segment` of line `index` is indented by:
    /// none for its first, the hang for the rest.
    pub(super) fn row_hang(&self, index: usize, segment: usize) -> usize {
        match segment {
            0 => 0,
            _ => self.hang_of(index),
        }
    }

    /// The screen row of line `index` column `col` is on, and where that row
    /// starts and ends (one past its last character).
    pub(super) fn segment_at(&self, index: usize, col: usize) -> (usize, usize, usize) {
        let starts = self.segments(index);
        let length = self.buffer().line_len_of(index);
        let at = starts.iter().rposition(|start| *start <= col).unwrap_or(0);
        let end = starts.get(at + 1).copied().unwrap_or(length);
        (at, starts[at], end)
    }

    /// The cells the characters of line `index` from `from` up to `to` take.
    pub(super) fn cells_between(&self, index: usize, from: usize, to: usize) -> usize {
        self.char_widths(index).into_iter().skip(from).take(to.saturating_sub(from)).sum()
    }

    /// The character of line `index` between columns `start` and `end`
    /// whose cells reach past `cells` from `start`: what a screen row's
    /// column lands on.
    fn col_at_cells(&self, index: usize, start: usize, end: usize, cells: usize) -> usize {
        let mut used = 0;
        for (col, width) in self.char_widths(index).into_iter().enumerate().skip(start).take(end - start) {
            if used + width > cells {
                return col;
            }
            used += width;
        }
        end
    }

    /// A key that moves over the folds, while lines are folded: what it
    /// does, or `None` for one the modal keys read as always. `goal` is the
    /// column within its row the last vertical move kept to.
    pub(super) fn wrap_key(&mut self, key: &Key, goal: Option<usize>) -> Option<EditorOutcome> {
        self.fold?;
        // The exact chord: `C-S-f` is the window's file browser, not `C-f`
        // with Shift to spare, and `C-S-v` its paste.
        let chord = key.chord();
        let page = self.viewport.max(1) as isize;
        if self.modal.is_typing() {
            let rows = match chord {
                (Mods::NONE, KeyCode::Down) | (Mods::CTRL, KeyCode::Char('n')) => 1,
                (Mods::NONE, KeyCode::Up) | (Mods::CTRL, KeyCode::Char('p')) => -1,
                (Mods::CTRL, KeyCode::Char('v')) => page - 2,
                (Mods::ALT, KeyCode::Char('v')) => -(page - 2),
                (Mods::NONE, KeyCode::PageDown) => page,
                (Mods::NONE, KeyCode::PageUp) => -page,
                (Mods::NONE, KeyCode::Home) | (Mods::CTRL, KeyCode::Char('a')) => {
                    return Some(self.goto_row_edge(false))
                }
                (Mods::NONE, KeyCode::End) | (Mods::CTRL, KeyCode::Char('e')) => {
                    return Some(self.goto_row_edge(true))
                }
                _ => return None,
            };
            self.move_screen_rows(rows, goal);
            return Some(EditorOutcome::Consumed);
        }
        if !matches!(self.modal.mode(), EditMode::Normal | EditMode::Visual) {
            return None;
        }
        if let (Some(Pending::Goto), (Mods::NONE, glyph)) = (self.modal.pending(), chord) {
            return self.goto_on_row(glyph);
        }
        if !self.modal.takes_motion() {
            return None;
        }
        let (rows, counted) = match chord {
            (Mods::NONE, KeyCode::Char('j') | KeyCode::Down) => (1, true),
            (Mods::NONE, KeyCode::Char('k') | KeyCode::Up) => (-1, true),
            (Mods::CTRL, KeyCode::Char('d')) => (page / 2, false),
            (Mods::CTRL, KeyCode::Char('u')) => (-(page / 2), false),
            (Mods::CTRL, KeyCode::Char('f')) | (Mods::NONE, KeyCode::PageDown) => (page, false),
            (Mods::CTRL, KeyCode::Char('b')) | (Mods::NONE, KeyCode::PageUp) => (-page, false),
            (Mods::NONE, KeyCode::Char(c @ ('H' | 'M' | 'L'))) => {
                let count = self.modal.take_count();
                return Some(self.goto_screen_row(c, count));
            }
            _ => return None,
        };
        let count = self.modal.take_count().unwrap_or(1) as isize;
        let rows = match counted {
            true => rows * count,
            false => rows,
        };
        self.move_screen_rows(rows, goal);
        Some(EditorOutcome::Consumed)
    }

    /// `rows` screen rows down (up where negative), keeping to `goal`, the
    /// column within the row the run of vertical moves started at.
    fn move_screen_rows(&mut self, rows: isize, goal: Option<usize>) {
        let (mut line, col) = (self.buffer().row(), self.buffer().col());
        let (mut segment, start, _) = self.segment_at(line, col);
        // The goal is a screen column, so a move onto an indented row keeps
        // to where the eye was.
        let goal = goal.unwrap_or_else(|| self.row_hang(line, segment) + self.cells_between(line, start, col));
        let total = self.buffer().lines().len();
        for _ in 0..rows.unsigned_abs() {
            if rows > 0 {
                if segment + 1 < self.segments(line).len() {
                    segment += 1;
                } else if line + 1 < total {
                    line += 1;
                    segment = 0;
                } else {
                    break;
                }
            } else if segment > 0 {
                segment -= 1;
            } else if line > 0 {
                line -= 1;
                segment = self.segments(line).len() - 1;
            } else {
                break;
            }
        }
        let starts = self.segments(line);
        let length = self.buffer().line_len_of(line);
        let start = starts[segment];
        let last = segment + 1 == starts.len();
        let end = starts.get(segment + 1).copied().unwrap_or(length);
        let cells = goal.saturating_sub(self.row_hang(line, segment));
        let mut col = self.col_at_cells(line, start, end, cells);
        // Short of the row's last, the cursor stays on the row, off the
        // character the next row starts with.
        if !last && col >= end {
            col = end.saturating_sub(1).max(start);
        }
        match self.modal.is_typing() {
            true => self.buffer_mut().move_to_typing(line, col),
            false => self.buffer_mut().move_to(line, col),
        }
        self.wrap_goal = Some(goal);
    }

    /// To the start of the cursor's screen row, or past its end (`end`):
    /// Emacs' `C-a` and `C-e` in `visual-line-mode`.
    fn goto_row_edge(&mut self, end: bool) -> EditorOutcome {
        let (line, col) = (self.buffer().row(), self.buffer().col());
        let (segment, start, stop) = self.segment_at(line, col);
        let last = segment + 1 == self.segments(line).len();
        let target = match (end, last) {
            (false, _) => start,
            (true, true) => stop,
            // The row's last character is the space it folded at: before it
            // is the row's end, as it reads.
            (true, false) => stop.saturating_sub(1).max(start),
        };
        self.buffer_mut().move_to_typing(line, target);
        EditorOutcome::Consumed
    }

    /// `g0`, `g^`, `gm`, `g$` on the cursor's screen row, and `gj` and `gk`
    /// by whole lines, with folding on. `None` for the other `g` commands.
    fn goto_on_row(&mut self, glyph: KeyCode) -> Option<EditorOutcome> {
        let (line, col) = (self.buffer().row(), self.buffer().col());
        let (_, start, end) = self.segment_at(line, col);
        let chars: Vec<char> = self.line_of(line).chars().collect();
        let target = match glyph {
            KeyCode::Char('0') => start,
            KeyCode::Char('^') => (start..end).find(|at| !chars[*at].is_whitespace()).unwrap_or(start),
            KeyCode::Char('$') => end.saturating_sub(1).max(start),
            KeyCode::Char('m') => {
                let half = self.fold.unwrap_or(0) / 2;
                self.col_at_cells(line, start, end, half)
            }
            KeyCode::Char(c @ ('j' | 'k')) => {
                let count = self.modal.take_count().unwrap_or(1);
                self.modal.clear_pending();
                match c {
                    'j' => self.buffer_mut().move_down(count, false),
                    _ => self.buffer_mut().move_up(count, false),
                }
                return Some(EditorOutcome::Consumed);
            }
            _ => return None,
        };
        self.modal.clear_pending();
        self.buffer_mut().move_to(line, target);
        Some(EditorOutcome::Consumed)
    }

    /// `H`, `M`, and `L` over the screen rows last painted: the top one (or
    /// the `count`th from it), the middle one, the bottom one (or the
    /// `count`th from it), kept inside the scroll margins as the view is,
    /// landing on its first character that is not a space.
    fn goto_screen_row(&mut self, which: char, count: Option<usize>) -> EditorOutcome {
        let shown = self.painted.len().min(self.viewport.max(1));
        if shown == 0 {
            return EditorOutcome::Consumed;
        }
        let nth = count.unwrap_or(1).saturating_sub(1).min(shown - 1);
        let edges = self.laid_out.shown_at;
        let at = match which {
            'H' => nth.max(edges.top),
            'M' => edges.middle,
            _ => (shown - 1 - nth).min(edges.bottom),
        }
        .min(shown - 1);
        let spot = self.painted[at];
        let start = self.char_at_display(spot.line, spot.offset);
        let (_, start, end) = self.segment_at(spot.line, start);
        let chars: Vec<char> = self.line_of(spot.line).chars().collect();
        let target = (start..end).find(|col| !chars[*col].is_whitespace()).unwrap_or(start);
        self.buffer_mut().move_to(spot.line, target);
        EditorOutcome::Consumed
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_line_folds_at_the_last_word_that_fits_by_the_cells_it_takes() {
        assert_eq!(fold_starts("short", 10, 0), [0]);
        assert_eq!(fold_starts("the quick brown fox", 10, 0), [0, 10]);
        assert_eq!(fold_starts("abcdefghijklmnop", 5, 0), [0, 5, 10, 15], "a long word breaks at the margin");
        assert_eq!(fold_starts("ab   cd", 3, 0), [0, 5], "the spaces stay on the row they follow");
        assert_eq!(fold_starts("中文中文", 5, 0), [0, 2], "two cells each");
        assert_eq!(fold_starts("", 5, 0), [0]);
        assert_eq!(fold_starts("abcdefghij", 6, 2), [0, 6], "the rows after the first narrower");
        assert_eq!(fold_starts("abcdefghijkl", 6, 2), [0, 6, 10]);
    }

    #[test]
    fn test_what_opens_a_line_is_read_by_its_kind() {
        let markdown = Some(Prose::Markdown);
        assert_eq!(semantic_lead("  - [ ] buy milk", markdown, None), 8);
        assert_eq!(semantic_lead("12. twelve", markdown, None), 4);
        assert_eq!(semantic_lead("### Heading", markdown, None), 4);
        assert_eq!(semantic_lead("> > quoted", markdown, None), 4);
        assert_eq!(semantic_lead("#hashtag", markdown, None), 0, "no space, no heading");
        assert_eq!(semantic_lead("    plain", markdown, None), 4, "else the indentation");
        assert_eq!(semantic_lead("  \\item[a] text", Some(Prose::Latex), None), 11);
        assert_eq!(semantic_lead("    /// docs here", None, Some("//")), 8);
        assert_eq!(semantic_lead("    let x = 1;", None, Some("//")), 4);
    }
}
