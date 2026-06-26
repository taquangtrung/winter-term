//! Reading aids over the visible grid: sentence-highlight spans (alternating
//! background bands, one per sentence, for reading agent/chat transcripts) and
//! rainbow-paren depth marks (bracket glyphs colored by nesting depth).
//!
//! Both are pure grid → marks functions, computed over the viewport each frame
//! on the app's render path (like the search match cells) and handed to the
//! renderer as per-cell data: the renderer stays free of segmentation and
//! depth logic.

use winter_render::{Grid, Theme};

/// How many rows above the viewport top both features scan (counting only, no
/// marks) so a sentence begun, or a bracket opened, on rows that have since
/// scrolled partially out still resolves correctly against what's visible.
const LOOKBACK_ROWS: usize = 40;

/// Bracket pairs whose glyphs get depth-colored. Angle brackets are excluded:
/// in terminal prose (`->`, `<html>`, generics printed by compilers) they pair
/// far less reliably than the three true bracket families.
const OPEN_TO_CLOSE: &[(char, char)] = &[('(', ')'), ('[', ']'), ('{', '}')];

// ========================================================================
// Data Structures
// ========================================================================

/// One sentence-highlight band clipped to a single viewport row: columns
/// `start..end` (exclusive) of `row` belong to the sentence of parity `tone`
/// (0 or 1, alternating across consecutive sentences).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SentenceSpan {
    pub col_end: usize,
    pub col_start: usize,
    pub row: usize,
    pub tone: u8,
}

/// A bracket glyph to recolor: viewport `(row, col)` and its nesting depth
/// (0-based, so a top-level `(` and its `)` both carry depth 0).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BracketMark {
    pub col: usize,
    pub depth: usize,
    pub row: usize,
}

// ========================================================================
// Line assembly
// ========================================================================

/// One assembled logical line: its chars plus, per char, the viewport
/// `(row, col)` it came from. Soft-wrapped continuation rows (see
/// [`Grid::row_wraps`]) are joined with their head row, so a sentence flowing
/// across a soft wrap maps back onto the right cells.
struct LogicalLine {
    cells: Vec<(char, usize, usize)>,
    /// True when the line contains no printable character.
    blank: bool,
}

/// Assemble `grid`'s viewport into logical lines: soft-wrap spans joined,
/// trailing blanks trimmed per row (see [`super::vim::line_chars`]).
fn logical_lines(grid: &Grid) -> Vec<LogicalLine> {
    let rows = grid.rows();
    let mut lines = Vec::new();
    let mut row = 0;
    while row < rows {
        let (first, last) = grid.wrapped_row_span(row);
        let mut cells: Vec<(char, usize, usize)> = Vec::new();
        for r in first..=last {
            for (col, ch) in super::vim::line_chars(grid, r).into_iter().enumerate() {
                cells.push((ch, r, col));
            }
        }
        let blank = cells.iter().all(|&(ch, _, _)| ch.is_whitespace());
        lines.push(LogicalLine { blank, cells });
        row = last + 1;
    }
    lines
}

// ========================================================================
// Sentence spans
// ========================================================================

/// The alternating sentence bands covering `grid`'s viewport, as row-clipped
/// column spans. Sentences are found with the classic heuristic the
/// terminal-vim addon also reduces to once its lowercase-masking runs: a break
/// happens at a terminator (`.`, `!`, `?`, plus any wrapping quotes/brackets)
/// only when the next non-blank text starts with an uppercase letter or digit
///: a lowercase continuation ("e.g. this") never breaks, which is what keeps
/// abbreviations and camelCase tail fragments from shattering a sentence.
/// Adjacent non-blank lines join into one paragraph (a CLI's hard wraps are
/// indistinguishable from paragraph flow except at blank lines); a sentence
/// may therefore span rows, clipped per row into [`SentenceSpan`]s.
pub(crate) fn sentence_spans(grid: &Grid) -> Vec<SentenceSpan> {
    let lines = logical_lines(grid);
    if lines.iter().all(|l| l.blank) {
        return vec![];
    }

    // Paragraph assembly: non-blank adjacent lines join; a blank line ends the
    // paragraph (and itself belongs to no sentence).
    let mut paragraphs: Vec<Vec<&LogicalLine>> = Vec::new();
    for line in &lines {
        if line.blank {
            paragraphs.push(Vec::new());
        } else {
            match paragraphs.last_mut() {
                Some(last) if !last.is_empty() => last.push(line),
                _ => paragraphs.push(vec![line]),
            }
        }
    }

    let mut spans = Vec::new();
    for paragraph in paragraphs {
        if paragraph.is_empty() {
            continue;
        }
        let chars: Vec<(char, usize, usize)> = paragraph
            .iter()
            .flat_map(|l| l.cells.iter().copied())
            .collect();
        emit_sentence_spans(&chars, &mut spans);
    }
    spans
}

/// Walk one paragraph's chars, appending the row-clipped spans of each
/// sentence found. The state machine: after a terminator (+ wrapping
/// quotes/brackets), a following run of blanks followed by an
/// uppercase/digit char starts the next sentence; anything else (lowercase,
/// more terminator-adjacent punctuation) continues the current one.
fn emit_sentence_spans(chars: &[(char, usize, usize)], spans: &mut Vec<SentenceSpan>) {
    let is_terminator = |c: char| matches!(c, '.' | '!' | '?');
    let is_wrap = |c: char| matches!(c, '"' | '\'' | '`' | ')' | ']' | '}' | '»' | '…');

    let mut tone: u8 = 0;
    // The char index the current sentence started at (already past leading
    // blanks), or None while skipping blanks before a first sentence.
    let mut start: Option<usize> = if chars.first().is_some_and(|&(c, _, _)| !c.is_whitespace()) {
        Some(0)
    } else {
        None
    };
    // Char index just past a terminator that may yet grow wrapping punctuation.
    let mut after_terminator: Option<usize> = None;
    let mut i = 0;
    while i < chars.len() {
        let (ch, _, _) = chars[i];
        if let Some(bound) = after_terminator {
            // Decide whether the text starting after `bound` opens a new
            // sentence: blanks then an uppercase/digit char does; any other
            // printable (a lowercase word) folds back into the current one.
            if ch.is_whitespace() {
                i += 1;
                continue;
            }
            if ch.is_uppercase() || ch.is_ascii_digit() {
                close_span(chars, start, bound, tone, spans);
                tone ^= 1;
                start = Some(i);
            }
            after_terminator = None;
            continue;
        }
        if start.is_none() {
            if ch.is_whitespace() {
                i += 1;
                continue;
            }
            start = Some(i);
        }
        if is_terminator(ch) {
            after_terminator = Some(i + 1);
        } else if is_wrap(ch) && start.is_some() {
            // A closing quote/bracket right after a terminator run: extend the
            // pending boundary past it. (A wrap char with no terminator behind
            // it is ordinary punctuation and simply continues the sentence.)
            let prev_terminated = i > 0
                && chars[..i]
                    .iter()
                    .rev()
                    .take_while(|&&(c, _, _)| is_wrap(c) || is_terminator(c))
                    .any(|&(c, _, _)| is_terminator(c));
            if prev_terminated {
                after_terminator = Some(i + 1);
            }
        }
        i += 1;
    }
    close_span(chars, start, chars.len(), tone, spans);
}

/// Append the spans for one sentence covering `chars[start..end)`, clipped per
/// viewport row. `start == None` (no sentence opened) appends nothing.
fn close_span(
    chars: &[(char, usize, usize)],
    start: Option<usize>,
    end: usize,
    tone: u8,
    spans: &mut Vec<SentenceSpan>,
) {
    let Some(start) = start else { return };
    if end <= start {
        return;
    }
    // Trim trailing blanks off the sentence (they belong to no band).
    let end = chars[start..end]
        .iter()
        .rposition(|&(c, _, _)| !c.is_whitespace())
        .map(|last| start + last + 1)
        .unwrap_or(start);
    if end <= start {
        return;
    }
    let mut row_range: Option<(usize, usize, usize)> = None;
    // Every char of the sentence maps to a cell (whitespace included, so a
    // wrapped row's band reads continuously), accumulated into per-row spans.
    for &(_, row, col) in &chars[start..end] {
        match row_range {
            Some((r, cs, ce)) if r == row => row_range = Some((r, cs, ce.max(col + 1))),
            Some((r, cs, ce)) => {
                spans.push(SentenceSpan {
                    col_end: ce,
                    col_start: cs,
                    row: r,
                    tone,
                });
                row_range = Some((row, col, col + 1));
            }
            None => row_range = Some((row, col, col + 1)),
        }
    }
    if let Some((r, cs, ce)) = row_range {
        spans.push(SentenceSpan {
            col_end: ce,
            col_start: cs,
            row: r,
            tone,
        });
    }
}

// ========================================================================
// Rainbow parens
// ========================================================================

/// The bracket glyphs of `grid`'s viewport with their nesting depths: a
/// matching pair both carry the depth the opener had when it opened (0-based,
/// so the outermost pair shares depth 0). An unmatched closer carries
/// [`UNMATCHED_DEPTH`] instead. Rows above the viewport (up to
/// [`LOOKBACK_ROWS`]) are scanned depth-only so brackets opened off-screen
/// still resolve to their true depth.
pub(crate) const UNMATCHED_DEPTH: usize = usize::MAX;

pub(crate) fn bracket_marks(grid: &Grid) -> Vec<BracketMark> {
    let close_to_open: &[(char, char)] = &[(')', '('), (']', '['), ('}', '{')];

    // Depth-only pre-scan of the lookback region: count net opens so the
    // visible scan starts at the right depth. Unclosed lookback opens simply
    // raise the starting depth: the visible brackets still cycle correctly
    // relative to what's above.
    let top_abs = grid.to_absolute_row(0);
    let mut depth = 0usize;
    let lookback_start = top_abs.saturating_sub(LOOKBACK_ROWS);
    for abs_row in lookback_start..top_abs {
        for ch in row_chars_absolute(grid, abs_row) {
            if OPEN_TO_CLOSE.iter().any(|&(o, _)| o == ch) {
                depth += 1;
            } else if close_to_open.iter().any(|&(c, _)| c == ch) {
                depth = depth.saturating_sub(1);
            }
        }
    }

    // Visible scan: a stack of (open_char, depth_at_open) pairs colors each
    // closer with its own opener's depth, mirroring the addon's
    // `_findBracketColors`.
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut marks = Vec::new();
    for row in 0..grid.rows() {
        for (col, ch) in super::vim::line_chars(grid, row).into_iter().enumerate() {
            if let Some(&(_, _)) = OPEN_TO_CLOSE.iter().find(|&&(o, _)| o == ch) {
                let at = stack.len() + depth;
                stack.push((ch, at));
                marks.push(BracketMark {
                    col,
                    depth: at,
                    row,
                });
            } else if let Some(&(_, open)) = close_to_open.iter().find(|&&(c, _)| c == ch) {
                match stack.last() {
                    Some((top_open, at)) if *top_open == open => {
                        let at = *at;
                        stack.pop();
                        marks.push(BracketMark {
                            col,
                            depth: at,
                            row,
                        });
                    }
                    _ => marks.push(BracketMark {
                        col,
                        depth: UNMATCHED_DEPTH,
                        row,
                    }),
                }
            }
        }
    }
    marks
}

/// Resolve depth-marked bracket glyphs to per-cell RGB colors using the
/// theme's ANSI palette. Unmatched brackets take the theme's red (ANSI 1);
/// matched brackets cycle through yellow, magenta, cyan, green, and blue.
pub(crate) fn resolve_bracket_colors(
    marks: &[BracketMark],
    theme: &Theme,
) -> Vec<(usize, usize, (u8, u8, u8))> {
    const RAINBOW_CYCLE: &[usize] = &[3, 5, 6, 2, 4];
    marks
        .iter()
        .map(|m| {
            let rgb = if m.depth == UNMATCHED_DEPTH {
                theme.ansi[1]
            } else {
                let idx = RAINBOW_CYCLE[m.depth % RAINBOW_CYCLE.len()];
                theme.ansi[idx]
            };
            (m.row, m.col, (rgb.r, rgb.g, rgb.b))
        })
        .collect()
}

/// A row's printable chars by absolute row, for the lookback pre-scan (the
/// viewport API only addresses visible rows).
fn row_chars_absolute(grid: &Grid, abs_row: usize) -> Vec<char> {
    let mut chars = Vec::with_capacity(grid.cols());
    for col in 0..grid.cols() {
        if let Some(cell) = grid.absolute_cell(abs_row, col) {
            if cell.ch != '\0' && !cell.ch.is_whitespace() {
                chars.push(cell.ch);
            }
        }
    }
    chars
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid whose viewport holds `lines`, one per row, written straight into
    /// the grid (no shell, no timing).
    fn grid_from_lines(lines: &[&str]) -> Grid {
        let cols = lines
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(20)
            .max(20)
            + 5;
        let mut grid = Grid::new(cols, lines.len().max(6));
        for (row, line) in lines.iter().enumerate() {
            grid.move_to(row, 0);
            for ch in line.chars() {
                grid.print(ch);
            }
        }
        grid.move_to(0, 0);
        grid
    }

    /// A grid printing `text` at `cols` wide, so real soft wraps split it
    /// across rows exactly as terminal output would.
    fn grid_from_wrapped(text: &str, cols: usize) -> Grid {
        let mut grid = Grid::new(cols, 8);
        for ch in text.chars() {
            if ch == '\n' {
                grid.print(' ');
                continue;
            }
            grid.print(ch);
        }
        grid.move_to(0, 0);
        grid
    }

    /// The spans' text for one row, as `start..end` column ranges joined for
    /// readability.
    fn row_spans(spans: &[SentenceSpan], row: usize) -> Vec<(usize, usize, u8)> {
        spans
            .iter()
            .filter(|s| s.row == row)
            .map(|s| (s.col_start, s.col_end, s.tone))
            .collect()
    }

    #[test]
    fn test_two_sentences_alternate_tones_on_one_row() {
        let grid = grid_from_lines(&["One sentence here. Another one!"]);
        let spans = sentence_spans(&grid);
        assert_eq!(
            row_spans(&spans, 0),
            vec![(0, 18, 0), (19, 31, 1)],
            "terminator + space + capital splits, tones alternate"
        );
    }

    #[test]
    fn test_lowercase_continuation_does_not_split() {
        // "e.g. this" must stay one sentence: the lowercase `t` after the
        // blanks folds back instead of opening a new band.
        let grid = grid_from_lines(&["Config e.g. this way works."]);
        let spans = sentence_spans(&grid);
        assert_eq!(row_spans(&spans, 0), vec![(0, 27, 0)]);
    }

    #[test]
    fn test_sentence_flows_across_a_soft_wrap_but_not_a_blank_line() {
        // 20 cols: the first sentence really soft-wraps from row 0 into row 1
        // (one logical line, one band crossing both); the blank row then ends
        // the paragraph, and the final sentence gets the flipped tone.
        let grid = grid_from_wrapped(
            "A long sentence that keeps flowing on.\n\nNew paragraph.",
            20,
        );
        let spans = sentence_spans(&grid);
        let row0 = row_spans(&spans, 0);
        let row1 = row_spans(&spans, 1);
        assert_eq!(row0.first().map(|s| s.2), Some(0));
        assert_eq!(
            row1.first().map(|s| s.2),
            Some(0),
            "continuation row keeps the head row's sentence and tone"
        );
        assert!(
            row1.len() <= 1 || row1[0].2 == 0,
            "the wrapped sentence's rows all share one tone"
        );
        assert!(
            row_spans(&spans, 2).is_empty() || grid.row_wraps(1),
            "blank row region"
        );
        let last_row = spans.last().expect("the new paragraph's band");
        assert_eq!(last_row.tone, 1, "paragraph break flips the tone");
    }

    #[test]
    fn test_wrapping_quote_extends_the_boundary() {
        // The closing quote belongs to the first sentence; the capital after
        // the quote-terminated blanks starts the second.
        let grid = grid_from_lines(&["He said \"stop.\" Then left."]);
        let spans = sentence_spans(&grid);
        assert_eq!(row_spans(&spans, 0), vec![(0, 15, 0), (16, 26, 1)]);
    }

    #[test]
    fn test_digit_starts_a_new_sentence() {
        let grid = grid_from_lines(&["Version one. 2nd release here."]);
        let spans = sentence_spans(&grid);
        assert_eq!(row_spans(&spans, 0), vec![(0, 12, 0), (13, 30, 1)]);
    }

    #[test]
    fn test_bracket_marks_share_depth_within_a_pair() {
        let grid = grid_from_lines(&["f(a, g(b), [c])"]);
        let marks = bracket_marks(&grid);
        let at = |col| {
            marks
                .iter()
                .find(|m| m.col == col)
                .map(|m| m.depth)
                .expect("a mark at the column")
        };
        assert_eq!(at(1), 0, "outer ( opens at depth 0");
        assert_eq!(at(6), 1, "inner ( opens at depth 1");
        assert_eq!(at(8), 1, "its ) shares depth 1");
        assert_eq!(at(11), 1, "[ opens at depth 1");
        assert_eq!(at(13), 1, "] shares depth 1");
        assert_eq!(at(14), 0, "outer ) shares depth 0");
    }

    #[test]
    fn test_unmatched_closer_is_marked_unmatched() {
        let grid = grid_from_lines(&["just a stray ) here"]);
        let marks = bracket_marks(&grid);
        assert_eq!(
            marks,
            vec![BracketMark {
                row: 0,
                col: 13,
                depth: UNMATCHED_DEPTH
            }]
        );
    }

    #[test]
    fn test_depth_carries_across_rows() {
        let grid = grid_from_lines(&["open ( then", "close ) here"]);
        let marks = bracket_marks(&grid);
        assert_eq!(marks.len(), 2);
        assert_eq!(marks[0].depth, marks[1].depth);
        assert_eq!(marks[0].row, 0);
        assert_eq!(marks[1].row, 1);
    }

    #[test]
    fn test_resolve_bracket_colors_maps_depth_to_theme_ansi_and_unmatched_to_red() {
        let theme = Theme::default();
        let marks = vec![
            BracketMark {
                col: 1,
                depth: 0,
                row: 0,
            },
            BracketMark {
                col: 6,
                depth: 1,
                row: 0,
            },
            BracketMark {
                col: 13,
                depth: UNMATCHED_DEPTH,
                row: 0,
            },
        ];
        let colors = resolve_bracket_colors(&marks, &theme);
        assert_eq!(colors.len(), 3);
        assert_eq!(
            colors[0],
            (0, 1, (theme.ansi[3].r, theme.ansi[3].g, theme.ansi[3].b))
        );
        assert_eq!(
            colors[1],
            (0, 6, (theme.ansi[5].r, theme.ansi[5].g, theme.ansi[5].b))
        );
        assert_eq!(
            colors[2],
            (0, 13, (theme.ansi[1].r, theme.ansi[1].g, theme.ansi[1].b))
        );
    }
}
