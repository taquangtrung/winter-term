//! Painting the editor: its rows, what is selected, the caret's column, rich-
//! text copies, and keeping the cursor in view.

use std::time::Duration;

use winter_model::editor::{EditorOutcome, TargetStyle, TargetStyles};
use winter_model::paint::{GutterLane, GutterMark, Row, Span, Style, StyleRun, TAB_WIDTH};
use winter_model::textedit::modal::Selection;

use super::{guides, runs, swatch, Cell, TextEditor};

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
        // The server's inlay hints (see `hints`).
        let hints = match marker {
            true => Vec::new(),
            false => self.hints_after(index),
        };
        // The phrases lit, ahead of the sentence wash, which comes after them.
        let bands = match marker {
            true => Vec::new(),
            false => self.highlight_bands(index),
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
            let style = bands
                .iter()
                .find(|band| (band.start..band.end).contains(&offset))
                .map_or(style, |band| band.style);
            let squiggle = squiggles.get(offset).copied().flatten();
            let in_lit = lit
                .iter()
                .any(|(start, end)| (*start..*end).contains(&offset));
            // An occurrence being changed takes the band a name changed in
            // place wears; one only lit, a faint band under its own color.
            let (style, lit) = match (in_lit, changing) {
                (true, true) => (Style::Marked, false),
                (true, false) => (style, true),
                (false, _) => (style, false),
            };
            // The problem the pointer's card describes is outlined as an
            // occurrence is, in its own colors and under its own squiggle.
            let lit = lit
                || hovered
                    .iter()
                    .any(|(start, end)| (*start..*end).contains(&offset));
            match c == '\t' {
                true => cells.extend(std::iter::repeat_n((' ', style, squiggle, lit), TAB_WIDTH)),
                false => cells.push((c, style, squiggle, lit)),
            }
            // What the server says of the code here, after the character
            // it follows, dim.
            for (_, label) in hints.iter().filter(|(after, _)| *after == offset) {
                cells.extend(label.chars().map(|c| (c, Style::Dim, None, false)));
            }
        }
        // A faint line at each level of the indentation, in its blanks.
        if self.indent_guides && !marker {
            guides::mark(&mut cells, &guides::columns(self.buffer().lines(), index));
        }
        // The other cursors, each on the cell it stands on, past the line's
        // end in Insert mode where it is about to type.
        if !marker {
            for col in self.cursors_on(index) {
                let at = self.painted_cells_before(index, col);
                if cells.len() <= at {
                    cells.resize(at + 1, (' ', Style::Normal, None, false));
                }
                cells[at].1 = Style::Cursor;
            }
        }
        // Ghost text goes in at the cursor, dim, the line's own
        // text after it moved on past it.
        let ghost = match marker {
            true => None,
            false => self.ghost_cells(index),
        };
        let (ghost_at, ghost_len) = ghost
            .as_ref()
            .map_or((usize::MAX, 0), |(at, ghost)| (*at, ghost.len()));
        if let Some((at, ghost)) = ghost {
            let at = at.min(cells.len());
            cells.splice(at..at, ghost);
        }
        let from = match self.fold {
            Some(_) => 0,
            None => cells.len().min(self.doc().hscroll),
        };
        let flashing = index == self.buffer().row() && self.is_flashing();
        // The main cursor's selection, and each other cursor's.
        let marked: Vec<(usize, usize)> = match flashing {
            true => vec![(0, cells.len().saturating_sub(from).max(1))],
            false => self
                .selection()
                .into_iter()
                .chain(self.cursor_selections())
                .filter_map(|selection| self.columns_of(index, &selection))
                .map(|(start, end)| (start.saturating_sub(from), end.saturating_sub(from)))
                .collect(),
        };
        // Ghost text past its first line goes on rows of its own
        // under the line, scrolled sideways as the line is.
        let ghost_rows = match marker {
            true => Vec::new(),
            false => self.ghost_rows(index, cols, from),
        };
        if self.fold.is_none() {
            let mut row = Vec::new();
            row.extend(runs(&cells[from..], cols, &marked));
            let mut rows = vec![row];
            rows.extend(ghost_rows);
            return rows;
        }
        // Folded, the line is one row per screen row, each row's cells from
        // where its first character's cells start, the rest hung under the
        // first as the line's opening says.
        let cell_of = |col: usize| -> usize {
            let cell = self.painted_cells_before(index, col);
            match cell > ghost_at {
                true => cell + ghost_len,
                false => cell,
            }
        };
        let starts = self.segments(index);
        let hang = self.hang_of(index);
        let mut rows: Vec<Row> = starts
            .iter()
            .enumerate()
            .map(|(at, start)| {
                let from = cell_of(*start);
                let last = at + 1 == starts.len();
                let to = starts
                    .get(at + 1)
                    .map_or(cells.len(), |next| cell_of(*next));
                let piece = &cells[from.min(cells.len())..to.min(cells.len())];
                let marked: Vec<(usize, usize)> = marked
                    .iter()
                    .map(|(start, end)| {
                        let start = start.saturating_sub(from);
                        let end = end.saturating_sub(from);
                        // Short of the line's last row, a selection running on
                        // stops at the row's end rather than marking a cell past it.
                        match last {
                            true => (start, end),
                            false => (start.min(piece.len()), end.min(piece.len())),
                        }
                    })
                    .collect();
                let (lead, room) = match at {
                    0 => (Vec::new(), cols),
                    _ => (
                        vec![Span::new(Style::Dim, " ".repeat(hang))],
                        cols.saturating_sub(hang),
                    ),
                };
                let mut row = lead;
                row.extend(runs(piece, room, &marked));
                row
            })
            .collect();
        rows.extend(ghost_rows);
        rows
    }

    /// The display columns of line `index` `selection` covers, as a
    /// half-open range, or `None` where it covers none of them.
    pub(super) fn columns_of(&self, index: usize, selection: &Selection) -> Option<(usize, usize)> {
        if index < selection.start.0 || index > selection.end.0 {
            return None;
        }
        let width = self.painted_cells_before(index, self.buffer().line_len_of(index));
        if selection.linewise {
            return Some((0, width.max(1)));
        }
        // A block covers the same columns on every row, as far as the row reaches.
        if selection.block {
            let len = self.buffer().line_len_of(index);
            let start = self.display_of(index, selection.start.1.min(len));
            let end = self.display_of(index, (selection.end.1 + 1).min(len));
            return (start < end).then_some((start, end));
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

    /// The syntax colors of lines `rows`, for a list showing them (scout),
    /// each cut to its text past its indentation. Only those lines are
    /// colored: the lines above the deepest are learned (the cheap pass, no
    /// colors) as far as `budget` allows, and a line not reached by then is
    /// left pending for the next ask.
    pub(super) fn target_styles(&mut self, rows: &[usize], budget: Duration) -> TargetStyles {
        let Some(&deepest) = rows.iter().max() else {
            return TargetStyles::default();
        };
        let doc = self.doc_mut();
        if doc.grammar.is_none() {
            return TargetStyles::default();
        }
        doc.color_upto = doc.color_upto.max(deepest);
        doc.learn_states(budget);
        for &row in rows {
            doc.color_lines(row, row + 1);
        }
        let mut styles = TargetStyles::default();
        for &row in rows {
            let Some(line) = self.buffer().lines().get(row) else {
                continue;
            };
            if self.colors_of(row).is_empty() {
                styles.pending |= !line.is_empty();
                continue;
            }
            let text = line.trim();
            let lead = line.chars().take_while(|c| c.is_whitespace()).count();
            styles.styles.push(TargetStyle {
                runs: self.style_runs(row, lead, text.chars().count()),
                target: row,
            });
        }
        styles
    }

    /// The colors of `chars` characters of line `index` from character
    /// `from`, as runs, from what has been worked out: none for a line not
    /// colored yet.
    pub(super) fn style_runs(&self, index: usize, from: usize, chars: usize) -> Vec<StyleRun> {
        let mut runs: Vec<StyleRun> = Vec::new();
        for &style in self.colors_of(index).iter().skip(from).take(chars) {
            match runs.last_mut() {
                Some(run) if run.style == style => run.chars += 1,
                Some(_) | None => runs.push(StyleRun { chars: 1, style }),
            }
        }
        runs
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

    /// The painted cell the cursor is on: how far into the line it sits
    /// once tabs are opened out and the scroll taken off.
    pub(super) fn caret_col(&self) -> usize {
        match self.fold {
            Some(cols) => {
                let (row, col) = (self.buffer().row(), self.buffer().col());
                let (segment, start, _) = self.segment_at(row, col);
                let hang = self.row_hang(row, segment);
                hang + self
                    .cells_between(row, start, col)
                    .min(cols.saturating_sub(hang + 1))
            }
            None => self.display_col().saturating_sub(self.doc().hscroll),
        }
    }

    /// The gutter's lane of line numbers: as wide as the last line's number,
    /// so a short file's numbers sit close to the edge and its text.
    pub(super) fn number_lane(&self) -> GutterLane {
        let lines = self.buffer().lines().len().max(1);
        GutterLane::Text {
            width: lines.to_string().len(),
        }
    }

    /// Line `index`'s number, for the gutter: every number alike but the
    /// cursor's line's, whose color the theme may set. A problem on a line
    /// is marked on its text.
    pub(super) fn number_mark(&self, index: usize) -> GutterMark {
        let style = match index == self.buffer().row() {
            true => Style::LineNumberActive,
            false => Style::Dim,
        };
        GutterMark::Text(Span::new(style, (index + 1).to_string()))
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
