//! Painting the editor: its rows, what is selected, the caret's column, rich-
//! text copies, and keeping the cursor in view.

use std::time::Duration;

use crate::model::editor::EditorOutcome;
use crate::model::paint::{Row, Span, Style, TAB_WIDTH};

use super::{runs, swatch, Cell, TextEditor, NUMBER_GAP, NUMBER_WIDTH};

// ========================================================================
// TextEditor: painting
// ========================================================================

impl TextEditor {
    /// One line of the file: its number, then as much of its text as the pane
    /// has room for. The caret is the host's to draw, on the cell this editor
    /// reports, so it takes the shape and the color every other caret in the
    /// window has. A merge conflict's marker is drawn on the band a diff's
    /// hunk header sits on, so each side's own lines keep their colors.
    pub(super) fn text_rows(&self, index: usize, cols: usize, marker: bool) -> Vec<Row> {
        // Every number alike, but the cursor's line, whose color the theme
        // may set: a problem on a line is marked on its text.
        let number_style = match index == self.buffer().row() {
            true => Style::LineNumberActive,
            false => Style::Dim,
        };
        let number = Span::new(
            number_style,
            format!("{:>NUMBER_WIDTH$}{NUMBER_GAP}", index + 1),
        );
        let line = self.line_of(index);
        let colors = self.colors_of(index);
        let squiggles = self.squiggles(index, line.chars().count());
        let hovered = match marker {
            true => Vec::new(),
            false => self.hovered_problem_columns(index, line.chars().count()),
        };
        let (lit, changing) = match marker {
            true => (Vec::new(), false),
            false => self.lit_columns(index, &line),
        };
        // Tabs are opened out for the screen, and every column they open into
        // carries what the tab itself was, so a run of color survives one.
        let mut cells: Vec<Cell> = Vec::new();
        let swatches = match self.swatches && !marker {
            true => swatch::swatches(&line),
            false => Vec::new(),
        };
        // Markup's markers hide (see `conceal`): they take no cell, and the
        // text they mark is drawn bold or italic as they say.
        let concealed = match marker {
            true => None,
            false => self.concealed(index),
        };
        for (offset, c) in line.chars().enumerate() {
            if concealed.as_ref().is_some_and(|shown| shown.hides(offset)) {
                continue;
            }
            let swatched = swatches
                .iter()
                .find(|found| (found.start..found.end).contains(&offset));
            let style = match (marker, swatched) {
                (true, _) => Style::Hunk,
                (false, Some(found)) => found.style(),
                (false, None) => colors.get(offset).copied().unwrap_or_default(),
            };
            let style = match &concealed {
                Some(shown) => shown.styled(offset, style),
                None => style,
            };
            let squiggle = squiggles.get(offset).copied().flatten();
            let in_lit = lit.iter().any(|(start, end)| (*start..*end).contains(&offset));
            // An occurrence being changed takes the band a name changed in
            // place wears; one only lit, a faint band under its own color.
            let (style, lit) = match (in_lit, changing) {
                (true, true) => (Style::Marked, false),
                (true, false) => (style, true),
                (false, _) => (style, false),
            };
            // The problem the pointer's card describes is outlined as an
            // occurrence is, in its own colors and under its own squiggle.
            let lit = lit || hovered.iter().any(|(start, end)| (*start..*end).contains(&offset));
            match c == '\t' {
                true => cells.extend(std::iter::repeat_n((' ', style, squiggle, lit), TAB_WIDTH)),
                false => cells.push((c, style, squiggle, lit)),
            }
        }
        // Copilot's ghost text goes in at the cursor, dim, the line's own
        // text after it moved on past it.
        let ghost = match marker {
            true => None,
            false => self.ghost_cells(index),
        };
        let (ghost_at, ghost_len) = ghost.as_ref().map_or((usize::MAX, 0), |(at, ghost)| (*at, ghost.len()));
        if let Some((at, ghost)) = ghost {
            let at = at.min(cells.len());
            cells.splice(at..at, ghost);
        }
        let from = match self.fold {
            Some(_) => 0,
            None => cells.len().min(self.doc().hscroll),
        };
        let flashing = index == self.buffer().row() && self.is_flashing();
        let marked = match flashing {
            true => Some((0, cells.len().saturating_sub(from).max(1))),
            false => self
                .selected_columns(index)
                .map(|(start, end)| (start.saturating_sub(from), end.saturating_sub(from))),
        };
        if self.fold.is_none() {
            let mut row = vec![number];
            row.extend(runs(&cells[from..], cols, marked));
            return vec![row];
        }
        // Folded, the line is one row per screen row: the number on the
        // first, the gutter blank on the rest, each row's cells from where
        // its first character's cells start.
        let chars: Vec<char> = line.chars().collect();
        let cell_of = |col: usize| -> usize {
            let cell: usize = chars
                .iter()
                .take(col)
                .enumerate()
                .map(|(at, c)| match (concealed.as_ref().is_some_and(|shown| shown.hides(at)), *c == '\t') {
                    (true, _) => 0,
                    (false, true) => TAB_WIDTH,
                    (false, false) => 1,
                })
                .sum();
            match cell > ghost_at {
                true => cell + ghost_len,
                false => cell,
            }
        };
        let starts = self.segments(index);
        let hang = self.hang_of(index);
        let gutter = " ".repeat(NUMBER_WIDTH + NUMBER_GAP.len());
        starts
            .iter()
            .enumerate()
            .map(|(at, start)| {
                let from = cell_of(*start);
                let last = at + 1 == starts.len();
                let to = starts.get(at + 1).map_or(cells.len(), |next| cell_of(*next));
                let piece = &cells[from.min(cells.len())..to.min(cells.len())];
                let marked = marked.map(|(start, end)| {
                    let start = start.saturating_sub(from);
                    let end = end.saturating_sub(from);
                    // Short of the line's last row, a selection running on
                    // stops at the row's end rather than marking a cell past it.
                    match last {
                        true => (start, end),
                        false => (start.min(piece.len()), end.min(piece.len())),
                    }
                });
                let lead = match at {
                    0 => number.clone(),
                    _ => Span::new(Style::Dim, format!("{gutter}{}", " ".repeat(hang))),
                };
                let room = match at {
                    0 => cols,
                    _ => cols.saturating_sub(hang),
                };
                let mut row = vec![lead];
                row.extend(runs(piece, room, marked));
                row
            })
            .collect()
    }

    /// The display columns of line `index` a Visual selection covers, as a
    /// half-open range, or `None` where it covers none of them.
    pub(super) fn selected_columns(&self, index: usize) -> Option<(usize, usize)> {
        let selection = self.selection()?;
        if index < selection.start.0 || index > selection.end.0 {
            return None;
        }
        let width = self.painted_cells_before(index, self.buffer().line_len_of(index));
        if selection.linewise {
            return Some((0, width.max(1)));
        }
        let start = match index == selection.start.0 {
            true => self.display_of(index, selection.start.1),
            false => 0,
        };
        let end = match index == selection.end.0 {
            true => self.display_of(index, selection.end.1 + 1),
            false => width.max(1),
        };
        Some((start, end))
    }

    /// The style of each character of line `index`, as the last paint worked
    /// them out. A line not worked out yet, and every line of a file in no
    /// language this knows, is plain text.
    pub(super) fn colors_of(&self, index: usize) -> &[Style] {
        self.doc()
            .colors
            .get(index)
            .and_then(Option::as_deref)
            .unwrap_or_default()
    }

    /// The lines a selection covers, or the whole file, as painted rows and
    /// as text, to go on the clipboard as rich text.
    pub(super) fn copy_rich(&mut self) -> EditorOutcome {
        let total = self.buffer().lines().len();
        let (first, last) = match self.selection() {
            Some(selection) => (selection.start.0, selection.end.0),
            None => (0, total.saturating_sub(1)),
        };
        let doc = self.doc_mut();
        doc.color_upto = doc.color_upto.max(last + 1);
        doc.learn_states(Duration::MAX);
        doc.color_lines(first, last + 1);
        let rows: Vec<Row> = (first..=last.min(total.saturating_sub(1)))
            .map(|index| {
                let line = self.line_of(index);
                let colors = self.colors_of(index);
                let mut row: Row = Vec::new();
                for (offset, c) in line.chars().enumerate() {
                    let style = colors.get(offset).copied().unwrap_or_default();
                    match row.last_mut() {
                        Some(span) if span.style == style => span.text.push(c),
                        _ => row.push(Span::new(style, c.to_string())),
                    }
                }
                row
            })
            .collect();
        let text = self.buffer().lines()[first..=last.min(total.saturating_sub(1))].join("\n");
        self.leave_to_normal();
        EditorOutcome::CopyStyled { rows, text }
    }

    /// Say what the grammar makes of the character under the cursor: its
    /// scopes and the rows of the theme's scope table that color it, which
    /// is what the `syntax` colors are named by.
    pub(super) fn show_scopes(&mut self) -> EditorOutcome {
        self.inspect_scopes = !self.inspect_scopes;
        if self.inspect_scopes {
            let row = self.buffer().row();
            let doc = self.doc_mut();
            doc.color_upto = doc.color_upto.max(row);
            doc.learn_states(Duration::MAX);
        }
        EditorOutcome::Consumed
    }

    /// The painted cell the cursor is on: the gutter's width, plus how far
    /// into the line it sits once tabs are opened out and the scroll taken
    /// off.
    pub(super) fn caret_col(&self) -> usize {
        let gutter = NUMBER_WIDTH + NUMBER_GAP.len();
        match self.fold {
            Some(cols) => {
                let (row, col) = (self.buffer().row(), self.buffer().col());
                let (segment, start, _) = self.segment_at(row, col);
                let hang = self.row_hang(row, segment);
                gutter + hang + self.cells_between(row, start, col).min(cols.saturating_sub(hang + 1))
            }
            None => gutter + self.display_col().saturating_sub(self.doc().hscroll),
        }
    }

    pub(super) fn line_of(&self, index: usize) -> String {
        self.buffer()
            .lines()
            .get(index)
            .cloned()
            .unwrap_or_default()
    }

    /// The column the cursor paints at, which is not the column it edits at
    /// whenever a tab precedes it.
    pub(super) fn display_col(&self) -> usize {
        self.display_of(self.buffer().row(), self.buffer().col())
    }

    /// The column character `col` of line `index` paints at, once the tabs
    /// before it are opened out and each character before it takes the cells
    /// it is drawn in: two for a wide one, none for a combining mark.
    pub(super) fn display_of(&self, index: usize, col: usize) -> usize {
        self.char_widths(index).into_iter().take(col).sum()
    }

    /// The character of line `index` painted at display column `display`,
    /// which is what a click on that cell is pointing at: the one whose
    /// cells reach past it, so a click on either half of a wide character
    /// lands on that character.
    pub(super) fn char_at_display(&self, index: usize, display: usize) -> usize {
        let mut at = 0;
        for (col, width) in self.char_widths(index).into_iter().enumerate() {
            if at + width > display {
                return col;
            }
            at += width;
        }
        self.buffer().line_len_of(index)
    }

    /// Keep the cursor's column on screen for a line wider than the pane.
    /// With wrapping on there is nothing to scroll: every column is painted.
    ///
    /// The view is scrolled sideways only while the cursor is past the
    /// pane's edge: one that fits again from the line's start brings it all
    /// the way back, so leaving the end of a long line (a paragraph of
    /// LaTeX, say) does not leave every shorter line scrolled out of sight.
    pub(super) fn follow_cursor(&mut self, cols: usize, wrap: bool) {
        if wrap || cols == 0 {
            self.doc_mut().hscroll = 0;
            return;
        }
        let col = self.display_col();
        if col < cols {
            self.doc_mut().hscroll = 0;
        } else if col < self.doc().hscroll {
            self.doc_mut().hscroll = col;
        } else if col >= self.doc().hscroll + cols {
            self.doc_mut().hscroll = col + 1 - cols;
        }
    }
}
