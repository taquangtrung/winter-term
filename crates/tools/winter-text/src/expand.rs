//! Expand selection, as VS Code's smart select and Emacs' expand-region:
//! `S-M-<right>` grows the selection to the next piece of code holding it
//! (a word, a string, what brackets hold and then the brackets too, a line,
//! a paragraph, the file), and `S-M-<left>` gives the last growth back.
//!
//! Where a language server serving the file can say (`selectionRange`), its
//! pieces are the ones grown through: an expression, an argument, a block, a
//! function. Elsewhere the editor finds them in the text itself.

use serde_json::Value;
use winter_lsp::protocol::{self, Position};
use winter_model::editor::{EditorOutcome, Notice};

use super::complete::is_word_char;
use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// What a server says it can do, for expanding a selection.
const PROVIDER: &str = "selectionRangeProvider";

/// The characters a string is quoted in.
const QUOTES: [char; 3] = ['"', '\'', '`'];

/// The brackets, each opener beside its closer.
const BRACKETS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

/// How many characters a bracket is looked for over, so a long file is not
/// read whole on every growth.
const SCAN_LIMIT: usize = 100_000;

/// What is said where the whole file is selected already, or nothing was
/// grown yet to give back.
const WHOLE_FILE: &str = "the whole file is selected";
const NOTHING_TO_SHRINK: &str = "nothing grown to give back";

// ========================================================================
// Data Structures
// ========================================================================

/// A piece of the text: its first character and its last, both taken.
pub(super) type Piece = ((usize, usize), (usize, usize));

/// The pieces a selection grows through, and which of them is selected.
#[derive(Debug, Default)]
pub(super) struct Expansion {
    at: usize,
    pieces: Vec<Piece>,
    /// The text's revision the pieces are of.
    revision: usize,
}

// ========================================================================
// TextEditor: expanding the selection
// ========================================================================

impl TextEditor {
    /// Grow the selection to the next piece holding it, asking the server
    /// for the pieces first where one can say.
    pub(super) fn expand_selection(&mut self) -> EditorOutcome {
        if let Some(next) = self.growing().map(|at| at + 1) {
            let Some(piece) = self.expansion.pieces.get(next).copied() else {
                self.modal.report(Notice::info(WHOLE_FILE).unlogged());
                return EditorOutcome::Consumed;
            };
            self.expansion.at = next;
            self.select_piece(piece);
            return EditorOutcome::Consumed;
        }
        let from = self.current_piece();
        if self.ask_selection_ranges(from.0) {
            return EditorOutcome::Consumed;
        }
        let pieces = self.found_pieces(from);
        self.start_growing(from, pieces)
    }

    /// Give the last growth back: the piece selected before it.
    pub(super) fn shrink_selection(&mut self) -> EditorOutcome {
        let Some(at) = self.growing().filter(|at| *at > 0) else {
            self.modal
                .report(Notice::info(NOTHING_TO_SHRINK).unlogged());
            return EditorOutcome::Consumed;
        };
        self.expansion.at = at - 1;
        let piece = self.expansion.pieces[at - 1];
        self.select_piece(piece);
        EditorOutcome::Consumed
    }

    /// Take the pieces a server says hold the selection's start, grown
    /// from `from`, and select the first.
    pub(super) fn take_selection_ranges(
        &mut self,
        (revision, from): (usize, Piece),
        value: &Value,
    ) -> EditorOutcome {
        if revision != self.buffer().revision() {
            return EditorOutcome::Consumed;
        }
        let lines = self.buffer().lines();
        let mut pieces = Vec::new();
        let mut range = value.as_array().and_then(|ranges| ranges.first());
        while let Some(at) = range {
            if let Some(piece) = at.get("range").and_then(|range| piece_of(lines, range)) {
                pieces.push(piece);
            }
            range = at.get("parent");
        }
        pieces.push(whole_file(lines));
        self.start_growing(from, pieces)
    }

    /// Start growing from `from` through `pieces`, keeping those that hold
    /// it and each other, and select the first.
    fn start_growing(&mut self, from: Piece, pieces: Vec<Piece>) -> EditorOutcome {
        let mut grown: Vec<Piece> = vec![from];
        for piece in pieces {
            let last = *grown.last().unwrap_or(&from);
            if holds(piece, last) && piece != last {
                grown.push(piece);
            }
        }
        if grown.len() < 2 {
            self.modal.report(Notice::info(WHOLE_FILE).unlogged());
            return EditorOutcome::Consumed;
        }
        self.expansion = Expansion {
            at: 1,
            pieces: grown,
            revision: self.buffer().revision(),
        };
        self.select_piece(self.expansion.pieces[1]);
        EditorOutcome::Consumed
    }

    /// Which piece is selected, where the selection is still one this grew
    /// to and the text has not changed under it.
    fn growing(&self) -> Option<usize> {
        let expansion = &self.expansion;
        let selected = self.selection()?;
        let piece = expansion.pieces.get(expansion.at)?;
        (expansion.revision == self.buffer().revision()
            && !selected.linewise
            && !selected.block
            && (selected.start, selected.end) == *piece)
            .then_some(expansion.at)
    }

    /// What is selected now as a piece: the selection, or the cursor's
    /// character alone.
    fn current_piece(&self) -> Piece {
        match self.selection() {
            Some(selection) => (selection.start, selection.end),
            None => {
                let at = (self.buffer().row(), self.buffer().col());
                (at, at)
            }
        }
    }

    /// Select `piece`, from its first character to its last.
    fn select_piece(&mut self, (start, end): Piece) {
        self.modal.clear_selection();
        self.buffer_mut().move_to(start.0, start.1);
        let at = self.at.min(self.docs.len() - 1);
        self.modal.start_visual(&self.docs[at].buffer, false);
        self.buffer_mut().move_to(end.0, end.1);
    }

    /// Ask the server serving the file for the pieces holding `at`. Says
    /// whether it was asked.
    fn ask_selection_ranges(&mut self, at: (usize, usize)) -> bool {
        let Some(port) = self
            .serving_port()
            .filter(|port| port.provides(PROVIDER))
            .cloned()
        else {
            return false;
        };
        let position = protocol::position_in(self.buffer().lines(), at.0, at.1);
        let params = serde_json::json!({
            "textDocument": {"uri": protocol::uri_of(&self.doc().path)},
            "positions": [{"line": position.line, "character": position.character}],
        });
        let Some(id) = port.request("textDocument/selectionRange", params) else {
            return false;
        };
        let from = self.current_piece();
        let revision = self.buffer().revision();
        self.wait_for_selection_ranges(port.id(), id, (revision, from));
        true
    }

    /// The pieces the text itself shows hold `from`, smallest first.
    fn found_pieces(&self, from: Piece) -> Vec<Piece> {
        let lines: Vec<Vec<char>> = self
            .buffer()
            .lines()
            .iter()
            .map(|line| line.chars().collect())
            .collect();
        let mut pieces = Vec::new();
        let (row, col) = from.0;
        if let Some(word) = word_at(&lines[row], col) {
            pieces.push(((row, word.0), (row, word.1)));
        }
        pieces.extend(
            quoted_around(&lines[row], from)
                .into_iter()
                .flat_map(|(open, close)| {
                    [
                        ((row, open + 1), (row, close - 1)),
                        ((row, open), (row, close)),
                    ]
                }),
        );
        let mut inside = from;
        while let Some((open, close)) = brackets_around(&lines, inside) {
            if let Some(inner) = inner_of(&lines, open, close) {
                pieces.push(inner);
            }
            pieces.push((open, close));
            inside = (open, close);
        }
        let last_row = lines.len() - 1;
        for row in [from.0 .0, from.1 .0] {
            let line = &lines[row];
            let first = line.iter().position(|c| !c.is_whitespace());
            if let (Some(first), Some(last)) =
                (first, line.iter().rposition(|c| !c.is_whitespace()))
            {
                pieces.push(((row, first), (row, last)));
            }
        }
        let (top, bottom) = paragraph_around(&lines, from);
        pieces.push(((top, 0), (bottom, lines[bottom].len().saturating_sub(1))));
        pieces.push(((0, 0), (last_row, lines[last_row].len().saturating_sub(1))));
        pieces.sort_by_key(|piece| size_of(&lines, *piece));
        pieces
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Whether `outer` holds every character `inner` does.
fn holds(outer: Piece, inner: Piece) -> bool {
    outer.0 <= inner.0 && inner.1 <= outer.1
}

/// How many characters `piece` takes, the line breaks counted.
fn size_of(lines: &[Vec<char>], (start, end): Piece) -> usize {
    match start.0 == end.0 {
        true => end.1.saturating_sub(start.1) + 1,
        false => {
            let middle: usize = (start.0 + 1..end.0).map(|row| lines[row].len() + 1).sum();
            (lines[start.0].len() - start.1.min(lines[start.0].len())) + 1 + middle + end.1 + 1
        }
    }
}

/// The word `col` is in, its first column and its last.
fn word_at(line: &[char], col: usize) -> Option<(usize, usize)> {
    if !line.get(col).is_some_and(|c| is_word_char(*c)) {
        return None;
    }
    let start = line[..col]
        .iter()
        .rposition(|c| !is_word_char(*c))
        .map_or(0, |at| at + 1);
    let end = line[col..]
        .iter()
        .position(|c| !is_word_char(*c))
        .map_or(line.len(), |at| col + at);
    Some((start, end - 1))
}

/// The quotes on `piece`'s line holding it, nearest first: each pair's
/// opening column and closing one. Only a piece on one line is quoted.
fn quoted_around(line: &[char], (start, end): Piece) -> Vec<(usize, usize)> {
    if start.0 != end.0 {
        return Vec::new();
    }
    let mut found = Vec::new();
    for quote in QUOTES {
        let at: Vec<usize> = (0..line.len()).filter(|at| line[*at] == quote).collect();
        for [open, close] in at.as_chunks::<2>().0.iter().copied() {
            if open < start.1 && end.1 < close {
                found.push((open, close));
            }
        }
    }
    found.sort_by_key(|(open, close)| close - open);
    found
}

/// The nearest brackets holding `piece` strictly: the opener before it,
/// and its closer after.
fn brackets_around(lines: &[Vec<char>], piece: Piece) -> Option<((usize, usize), (usize, usize))> {
    let mut depth = [0usize; BRACKETS.len()];
    let (mut row, mut col) = piece.0;
    for _ in 0..SCAN_LIMIT {
        match col.checked_sub(1) {
            Some(before) => col = before,
            None => {
                row = row.checked_sub(1)?;
                col = lines[row].len();
                continue;
            }
        }
        let c = lines[row][col];
        if let Some(kind) = BRACKETS.iter().position(|(_, close)| *close == c) {
            depth[kind] += 1;
        }
        if let Some(kind) = BRACKETS.iter().position(|(open, _)| *open == c) {
            match depth[kind] {
                0 => {
                    let close = closer_of(lines, (row, col), kind)?;
                    return (close >= piece.1 && (row, col) < piece.0)
                        .then_some(((row, col), close));
                }
                _ => depth[kind] -= 1,
            }
        }
    }
    None
}

/// Where the bracket opened at `open`, of kind `kind`, closes.
fn closer_of(lines: &[Vec<char>], open: (usize, usize), kind: usize) -> Option<(usize, usize)> {
    let (opener, closer) = BRACKETS[kind];
    let mut depth = 0usize;
    let (mut row, mut col) = open;
    for _ in 0..SCAN_LIMIT {
        col += 1;
        // Past the line's end, on to the next line holding anything.
        while col >= lines[row].len() {
            row += 1;
            col = 0;
            if row >= lines.len() {
                return None;
            }
        }
        let c = lines[row][col];
        if c == opener {
            depth += 1;
        } else if c == closer {
            match depth {
                0 => return Some((row, col)),
                _ => depth -= 1,
            }
        }
    }
    None
}

/// What the brackets at `open` and `close` hold, without them; `None`
/// where they hold nothing.
fn inner_of(lines: &[Vec<char>], open: (usize, usize), close: (usize, usize)) -> Option<Piece> {
    let start = match open.1 + 1 < lines[open.0].len() {
        true => (open.0, open.1 + 1),
        false => (open.0 + 1, 0),
    };
    let end = match close.1.checked_sub(1) {
        Some(before) => (close.0, before),
        None => {
            let row = close.0.checked_sub(1)?;
            (row, lines[row].len().saturating_sub(1))
        }
    };
    (start <= end).then_some((start, end))
}

/// The paragraph holding `piece`: the lines round it up to a blank one.
fn paragraph_around(lines: &[Vec<char>], (start, end): Piece) -> (usize, usize) {
    let blank = |row: usize| lines[row].iter().all(|c| c.is_whitespace());
    let mut top = start.0;
    while top > 0 && !blank(top - 1) {
        top -= 1;
    }
    let mut bottom = end.0;
    while bottom + 1 < lines.len() && !blank(bottom + 1) {
        bottom += 1;
    }
    (top, bottom)
}

/// The whole of `lines`.
fn whole_file(lines: &[String]) -> Piece {
    let last = lines.len().saturating_sub(1);
    let length = lines.get(last).map_or(0, |line| line.chars().count());
    ((0, 0), (last, length.saturating_sub(1)))
}

/// A range from the wire as a piece: its end, which the wire leaves out,
/// stepped back onto the last character it covers.
fn piece_of(lines: &[String], range: &Value) -> Option<Piece> {
    let at = |key: &str| -> Option<Position> {
        let point = range.get(key)?;
        Some(Position {
            character: u32::try_from(point.get("character")?.as_u64()?).ok()?,
            line: u32::try_from(point.get("line")?.as_u64()?).ok()?,
        })
    };
    let start = protocol::place_of(lines, at("start")?);
    let end = protocol::place_of(lines, at("end")?);
    let end = match end.1.checked_sub(1) {
        Some(before) => (end.0, before),
        None => {
            let row = end.0.checked_sub(1)?;
            (row, lines[row].chars().count().saturating_sub(1))
        }
    };
    (start <= end).then_some((start, end))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(text: &str) -> Vec<Vec<char>> {
        text.lines().map(|line| line.chars().collect()).collect()
    }

    #[test]
    fn test_the_brackets_round_a_piece_are_found_across_lines_past_others() {
        let lines = chars("f(a, [b, c],\n  g(d))");
        let inside_d = ((1, 4), (1, 4));
        assert_eq!(brackets_around(&lines, inside_d), Some(((1, 3), (1, 5))));
        let call = ((1, 3), (1, 5));
        assert_eq!(brackets_around(&lines, call), Some(((0, 1), (1, 6))));
        assert_eq!(brackets_around(&lines, ((0, 0), (1, 6))), None);
    }

    #[test]
    fn test_a_wire_range_s_end_steps_back_onto_its_last_character() {
        let lines = vec!["let x = 1;".to_string(), "y".to_string()];
        let range = serde_json::json!({
            "start": {"line": 0, "character": 4},
            "end": {"line": 0, "character": 5},
        });
        assert_eq!(piece_of(&lines, &range), Some(((0, 4), (0, 4))));
        let to_next = serde_json::json!({
            "start": {"line": 0, "character": 0},
            "end": {"line": 1, "character": 0},
        });
        assert_eq!(piece_of(&lines, &to_next), Some(((0, 0), (0, 9))));
    }
}
