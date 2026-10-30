//! Painting a diff session: each text a column (or a band, stacked), the
//! rows aligned across them, each region tinted in its text's hue, the
//! current one marked and its changed words standing out, and filler where
//! a text has no line.

use unicode_width::UnicodeWidthChar;

use crate::editors::word_diff;
use crate::model::paint::{Row, Span, Style};

use super::model::{Aligned, Session, Variant, A, ANCESTOR, B, C};

// ========================================================================
// Constants
// ========================================================================

/// What divides one column from the next.
pub(super) const DIVIDER: &str = "│";

/// What marks the rows of the current region, the rows of a merge region
/// still in conflict, and the rest, in the gutter.
const CURRENT_MARK: &str = "▶";
const CONFLICT_MARK: &str = "!";
const NO_MARK: &str = " ";

/// What a filler row shows, where a text has no line.
const FILLER: char = '╱';

// ========================================================================
// Data Structures
// ========================================================================

/// What painting a column needs of its text: its lines' colors.
pub(super) struct Column<'a> {
    pub(super) colors: &'a [Vec<Style>],
    pub(super) side: usize,
}

/// How wide each part of a column is.
#[derive(Clone, Copy, Debug)]
pub(super) struct Widths {
    /// The line number and the mark after it.
    pub(super) gutter: usize,
    /// The whole column.
    pub(super) total: usize,
}

// ========================================================================
// Functions
// ========================================================================

/// The tint a region's lines take in text `side`, and the louder one its
/// changed words take.
fn tints(side: usize) -> (Style, Style) {
    match side {
        A => (Style::Removed, Style::RemovedEdit),
        B => (Style::Added, Style::AddedEdit),
        C => (Style::Hunk, Style::Hunk),
        _ => (Style::Dim, Style::Dim),
    }
}

/// The header over a column: which text it is, what it is called, whether
/// it has edits not written, and whether it is the one being edited.
pub(super) fn header(session: &Session, side: usize, width: usize, editing: bool) -> Row {
    let letter = match side {
        A => "A",
        B => "B",
        C => "C",
        _ => "Ancestor",
    };
    let label = match side {
        ANCESTOR => session.ancestor.as_ref().map_or("", |side| side.label.as_str()),
        side => session.sides.get(side).map_or("", |side| side.label.as_str()),
    };
    let dirty = if session.is_dirty(side) { " [+]" } else { "" };
    let edit = if editing { "  (editing: C-c C-c ends)" } else { "" };
    let style = match editing {
        true => Style::HeaderAccent,
        false => Style::Header,
    };
    vec![Span::new(style, fit(&format!(" {letter}  {label}{dirty}{edit}"), width))]
}

/// One row's cells in one column: the line number, a mark for the current
/// region or a conflict, and the line's text from column `hscroll`, tinted
/// where it is in a region, or filler where the text has no line here.
pub(super) fn cell(
    session: &Session,
    row: &Aligned,
    column: &Column,
    widths: Widths,
    hscroll: usize,
    current: Option<usize>,
    refined: Option<&[Vec<word_diff::Segment>]>,
) -> Row {
    let side = column.side;
    let lines = session.lines(side);
    let digits = widths.gutter.saturating_sub(2);
    let text_width = widths.total.saturating_sub(widths.gutter);
    let region = row.region;
    let is_current = region.is_some() && region == current;
    let conflict = region.is_some_and(|index| {
        let region = &session.regions[index];
        region.conflict && region.variant == Some(Variant::Combined)
    });
    let mark = match (is_current, conflict) {
        (true, _) => (Style::Accent, CURRENT_MARK),
        (false, true) => (Style::Failed, CONFLICT_MARK),
        _ => (Style::Dim, NO_MARK),
    };
    let (tint, loud) = tints(side);
    let mut cells: Row = Vec::new();
    match row.lines[side] {
        None => {
            cells.push(Span::new(Style::Dim, " ".repeat(digits)));
            cells.push(Span::new(mark.0, mark.1));
            cells.push(Span::plain(" "));
            let filler = match region.is_some() {
                true => FILLER.to_string().repeat(text_width),
                false => " ".repeat(text_width),
            };
            cells.push(Span::new(Style::Dim, filler));
        }
        Some(line) => {
            let number = format!("{:>digits$}", line + 1);
            let number_style = match is_current {
                true => Style::Accent,
                false => Style::Dim,
            };
            cells.push(Span::new(number_style, number));
            cells.push(Span::new(mark.0, mark.1));
            cells.push(Span::plain(" "));
            let text = lines.get(line).map_or("", String::as_str);
            let styles = styles_of(text, line, column, region.map(|_| tint), is_current.then_some(loud), refined, row, session, side);
            cells.extend(clip(text, &styles, hscroll, text_width, region.map(|_| tint)));
        }
    }
    cells
}

/// The style of each character of `text`, line `line` of the column's text:
/// its syntax color outside a region, the region's tint inside one, and
/// in the current region its changed words louder where `refined` says
/// which they are.
#[allow(clippy::too_many_arguments)]
fn styles_of(
    text: &str,
    line: usize,
    column: &Column,
    tint: Option<Style>,
    loud: Option<Style>,
    refined: Option<&[Vec<word_diff::Segment>]>,
    row: &Aligned,
    session: &Session,
    side: usize,
) -> Vec<Style> {
    let count = text.chars().count();
    let Some(tint) = tint else {
        let colors = column.colors.get(line).map(Vec::as_slice).unwrap_or_default();
        return (0..count).map(|at| colors.get(at).copied().unwrap_or_default()).collect();
    };
    let mut styles = vec![tint; count];
    let (Some(loud), Some(refined), Some(index)) = (loud, refined, row.region) else {
        return styles;
    };
    // The refined lines are the region's lines of A, then of B.
    let region = &session.regions[index];
    let offset = match side {
        A => line.checked_sub(region.lines[A].start),
        B => line.checked_sub(region.lines[B].start).map(|at| at + region.lines[A].len()),
        _ => None,
    };
    let Some(segments) = offset.and_then(|at| refined.get(at)) else {
        return styles;
    };
    let mut at = 0;
    for segment in segments {
        let length = segment.text.chars().count();
        if segment.changed {
            for style in styles.iter_mut().skip(at).take(length) {
                *style = loud;
            }
        }
        at += length;
    }
    styles
}

/// `text` from display column `hscroll`, `width` columns of it, in runs of
/// one style, padded out to `width` in `pad` (the region's tint, so a
/// region reads as a band) or plain.
fn clip(text: &str, styles: &[Style], hscroll: usize, width: usize, pad: Option<Style>) -> Row {
    let mut spans: Row = Vec::new();
    let mut column = 0;
    let mut used = 0;
    for (c, style) in text.chars().zip(styles.iter().copied()) {
        let c = if c == '\t' { ' ' } else { c };
        let cells = c.width().unwrap_or(0);
        if column < hscroll {
            column += cells;
            continue;
        }
        if used + cells > width {
            break;
        }
        used += cells;
        column += cells;
        match spans.last_mut() {
            Some(last) if last.style == style => last.text.push(c),
            _ => spans.push(Span::new(style, c.to_string())),
        }
    }
    if used < width {
        spans.push(Span::new(pad.unwrap_or(Style::Normal), " ".repeat(width - used)));
    }
    spans
}

/// `text` cut or padded to `width` display columns.
pub(super) fn fit(text: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let cells = c.width().unwrap_or(0);
        if used + cells > width {
            break;
        }
        used += cells;
        out.push(c);
    }
    out.push_str(&" ".repeat(width.saturating_sub(used)));
    out
}

/// The word-level difference between a region's lines in A and in B, A's
/// lines first, for the current region's changed words.
pub(super) fn refine(session: &Session, index: usize) -> Vec<Vec<word_diff::Segment>> {
    let region = &session.regions[index];
    let mut marked: Vec<String> = session.lines(A)[region.lines[A].clone()]
        .iter()
        .map(|line| format!("-{line}"))
        .collect();
    marked.extend(session.lines(B)[region.lines[B].clone()].iter().map(|line| format!("+{line}")));
    word_diff::decorate(&marked)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::paint::row_text;

    #[test]
    fn test_a_cell_is_clipped_to_its_width_and_scrolled_sideways() {
        let styles = vec![Style::Normal; 10];
        assert_eq!(row_text(&clip("0123456789", &styles, 2, 5, None)), "23456");
        assert_eq!(row_text(&clip("ab", &styles, 0, 4, None)), "ab  ");
        let wide = vec![Style::Normal; 3];
        assert_eq!(row_text(&clip("中文字", &wide, 0, 5, None)), "中文 ", "a wide character never splits");
    }
}
