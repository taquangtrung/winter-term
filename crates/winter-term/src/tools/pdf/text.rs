//! A page's text as lines of characters, rebuilt from where its glyphs were
//! drawn.
//!
//! A PDF gives no lines, words, or spaces, only glyphs placed on the page.
//! The page is cut into blocks along the empty bands through it, so a
//! two-column paper reads down one column and then the other; each block's
//! glyphs are clustered into lines by baseline; and a space goes in wherever
//! two glyphs stand far enough apart to have had one.

use std::collections::HashSet;

// ========================================================================
// Constants
// ========================================================================

/// How far below the baseline a character's box reaches, as a share of its
/// font size: a box standing on the baseline would cut off every descender.
pub const DESCENT_RATIO: f32 = 0.22;

/// How close two glyphs' baselines have to be, relative to their size, to be
/// on the same line. A superscript sits far enough off to need the floor.
const LINE_Y_RATIO: f32 = 0.45;
const LINE_Y_FLOOR: f32 = 3.0;

/// How wide a gap between two glyphs has to be, relative to their size, to
/// read as a space. Kerning leaves gaps well under this; a space, well over.
const SPACE_GAP_RATIO: f32 = 0.15;

/// How much wider than the page's usual line gap a gap has to be to start a
/// paragraph.
const PARAGRAPH_GAP_MULTIPLIER: f32 = 1.6;

/// How tall an empty band across a block has to be, in the block's usual
/// glyph sizes, to cut it in two: well clear of a paragraph's gap, or two
/// columns would interleave a paragraph at a time.
const ROW_GAP_RATIO: f32 = 1.5;

/// How wide an empty band down a block has to be, by the same measure, to
/// cut it into columns: a gutter is about two sizes wide, an indent one.
const COLUMN_GAP_RATIO: f32 = 1.4;

/// How much of a block's text a column has to carry to stand on its own: a
/// list's bullets lined up, or a review copy's line numbers down the margin,
/// carry less and join the column beside them.
const COLUMN_SHARE: f32 = 0.2;

/// What share of a block's glyphs may cross a gutter and still leave it one:
/// a page number set under the gutter would otherwise close it.
const GUTTER_STRAY_SHARE: f32 = 0.002;

/// How many times a page is cut into smaller blocks: a banner over a
/// two-column body over a footnote is as deep as prose goes.
const MAX_CUT_DEPTH: usize = 4;

// ========================================================================
// Data Structures
// ========================================================================

/// One glyph as the page drew it, in the page's pixels at scale one, down
/// from its top-left.
#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    /// Its font size.
    pub height: f32,
    /// What it reads as: usually one character, a ligature several.
    pub text: String,
    /// How far it advances along the baseline.
    pub width: f32,
    /// Where its baseline starts.
    pub x: f32,
    pub y: f32,
}

/// A page's text, as lines in reading order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageText {
    /// The lines that start a paragraph.
    pub breaks: HashSet<usize>,
    pub lines: Vec<Line>,
}

/// One line: its characters, and a box for each.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// One per character of `text`.
    pub boxes: Vec<CharBox>,
    pub text: Vec<char>,
    /// Its baseline.
    pub y: f32,
}

/// Where a character stands: from its baseline's start, `width` along and
/// `height` its font size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharBox {
    pub height: f32,
    pub width: f32,
    pub x: f32,
    pub y: f32,
}

/// A run of glyphs while a line is gathered: its baseline and members.
struct Gathered {
    glyphs: Vec<Glyph>,
    y: f32,
}

// ========================================================================
// CharBox
// ========================================================================

impl CharBox {
    /// The box the glyph's ink falls in, as `(left, top, width, height)`:
    /// moved down off the baseline by a font's usual descent.
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        let top = self.y - self.height * (1.0 - DESCENT_RATIO);
        (self.x, top, self.width, self.height)
    }
}

// ========================================================================
// Line
// ========================================================================

impl Line {
    /// The box from the first character to the last, as `(left, top,
    /// width, height)`, or `None` for a line with none.
    pub fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let first = self.boxes.first()?.bounds();
        let last = self.boxes.last()?.bounds();
        Some(union(first, last))
    }
}

// ========================================================================
// PageText
// ========================================================================

impl PageText {
    /// The text of a page whose glyphs were drawn as `glyphs`.
    pub fn new(glyphs: Vec<Glyph>) -> Self {
        let glyphs: Vec<Glyph> = glyphs
            .into_iter()
            .filter(|glyph| !glyph.text.trim().is_empty())
            .collect();
        let lines: Vec<Line> = blocks(glyphs, 0)
            .into_iter()
            .flat_map(block_lines)
            .collect();
        Self {
            breaks: paragraph_breaks(&lines),
            lines,
        }
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// The smallest box holding both `(left, top, width, height)` boxes.
pub fn union(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    let left = a.0.min(b.0);
    let top = a.1.min(b.1);
    let right = (a.0 + a.2).max(b.0 + b.2);
    let bottom = (a.1 + a.3).max(b.1 + b.3);
    (left, top, right - left, bottom - top)
}

/// Cut `glyphs` into the blocks a reader takes them in: along a band across
/// first, which puts a banner ahead of the body, and only where none crosses
/// along a band down, the gutter of a two-column page.
fn blocks(glyphs: Vec<Glyph>, depth: usize) -> Vec<Vec<Glyph>> {
    if glyphs.len() < 2 || depth >= MAX_CUT_DEPTH {
        return vec![glyphs];
    }
    let size = median_height(&glyphs);
    if let Some((rows, _)) = cut(&glyphs, |g| (g.y - g.height, g.y), size * ROW_GAP_RATIO, 0) {
        return rows
            .into_iter()
            .filter(|row| !row.is_empty())
            .flat_map(|row| blocks(row, depth + 1))
            .collect();
    }
    let strays = (glyphs.len() as f32 * GUTTER_STRAY_SHARE) as usize;
    let span = |g: &Glyph| (g.x, g.x + g.width);
    if let Some((columns, gutters)) = cut(&glyphs, span, size * COLUMN_GAP_RATIO, strays) {
        let columns = merge_thin_columns(columns, gutters);
        if columns.len() > 1 {
            return columns
                .into_iter()
                .flat_map(|column| blocks(column, depth + 1))
                .collect();
        }
    }
    vec![glyphs]
}

/// Split `glyphs` along every band at least `least` wide in the interval
/// `span` takes off each that no more than `strays` of them cross, in
/// ascending order, with each band's width. A glyph crossing a band goes to
/// the side its start is on. `None` when no band is that wide.
fn cut(
    glyphs: &[Glyph],
    span: impl Fn(&Glyph) -> (f32, f32),
    least: f32,
    strays: usize,
) -> Option<(Vec<Vec<Glyph>>, Vec<f32>)> {
    let mut events: Vec<(f32, isize)> = glyphs
        .iter()
        .flat_map(|glyph| {
            let (start, end) = span(glyph);
            [(start, 1), (end, -1)]
        })
        .collect();
    events.sort_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)));
    let allowed = strays as isize;
    let mut crossing = 0;
    // Where the current band opened; none before the first glyph, so the
    // space outside the block is never a band.
    let mut opened: Option<f32> = None;
    let mut bands = Vec::new();
    for (at, step) in events {
        let before = crossing;
        crossing += step;
        if before > allowed && crossing <= allowed {
            opened = Some(at);
        } else if before <= allowed && crossing > allowed {
            if let Some(from) = opened.take() {
                if at - from >= least {
                    bands.push((from, at));
                }
            }
        }
    }
    if bands.is_empty() {
        return None;
    }
    let mut parts: Vec<Vec<Glyph>> = vec![Vec::new(); bands.len() + 1];
    for glyph in glyphs {
        let start = span(glyph).0;
        let part = bands
            .iter()
            .position(|(from, to)| start < (from + to) / 2.0)
            .unwrap_or(bands.len());
        parts[part].push(glyph.clone());
    }
    let widths = bands.iter().map(|(from, to)| to - from).collect();
    Some((parts, widths))
}

/// Fold every column carrying too little of the text into a neighbor,
/// across the narrower of its gutters, until each stands on its own or one
/// is left. `gutters[i]` is the width of the gutter right of `columns[i]`.
fn merge_thin_columns(mut columns: Vec<Vec<Glyph>>, mut gutters: Vec<f32>) -> Vec<Vec<Glyph>> {
    let total: usize = columns.iter().map(|column| char_count(column)).sum();
    while columns.len() > 1 {
        let Some((thinnest, chars)) = columns
            .iter()
            .map(|column| char_count(column))
            .enumerate()
            .min_by_key(|(_, chars)| *chars)
        else {
            break;
        };
        if chars as f32 >= total as f32 * COLUMN_SHARE {
            break;
        }
        let left = thinnest.checked_sub(1).map(|at| gutters[at]);
        let right = gutters.get(thinnest).copied();
        let into_left = match (left, right) {
            (Some(left), Some(right)) => left <= right,
            (left, _) => left.is_some(),
        };
        let column = columns.remove(thinnest);
        if into_left {
            columns[thinnest - 1].extend(column);
            gutters.remove(thinnest - 1);
        } else {
            columns[thinnest].extend(column);
            gutters.remove(thinnest);
        }
    }
    columns
}

/// Cluster one block's glyphs into lines by baseline, top to bottom.
fn block_lines(mut glyphs: Vec<Glyph>) -> Vec<Line> {
    glyphs.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
    let mut gathered: Vec<Gathered> = Vec::new();
    for glyph in glyphs {
        let tolerance = LINE_Y_FLOOR.max(glyph.height * LINE_Y_RATIO);
        match gathered
            .iter_mut()
            .find(|line| (line.y - glyph.y).abs() <= tolerance)
        {
            Some(line) => {
                line.y = (line.y + glyph.y) / 2.0;
                line.glyphs.push(glyph);
            }
            None => gathered.push(Gathered {
                y: glyph.y,
                glyphs: vec![glyph],
            }),
        }
    }
    gathered.sort_by(|a, b| a.y.total_cmp(&b.y));
    gathered
        .into_iter()
        .map(|mut line| {
            line.glyphs.sort_by(|a, b| a.x.total_cmp(&b.x));
            lay_out(&line.glyphs, line.y)
        })
        .collect()
}

/// One line's glyphs as its text and a box per character, with a space put
/// in wherever two glyphs stand far enough apart to have had one.
fn lay_out(glyphs: &[Glyph], y: f32) -> Line {
    let mut text = Vec::new();
    let mut boxes = Vec::new();
    for (index, glyph) in glyphs.iter().enumerate() {
        if let Some(previous) = index.checked_sub(1).map(|at| &glyphs[at]) {
            let end = previous.x + previous.width;
            let gap = glyph.x - end;
            if gap > previous.height * SPACE_GAP_RATIO {
                text.push(' ');
                boxes.push(CharBox {
                    height: previous.height,
                    width: gap,
                    x: end,
                    y: previous.y,
                });
            }
        }
        let count = glyph.text.chars().count().max(1);
        let each = glyph.width / count as f32;
        for (at, ch) in glyph.text.chars().enumerate() {
            text.push(ch);
            boxes.push(CharBox {
                height: glyph.height,
                width: each,
                x: glyph.x + each * at as f32,
                y: glyph.y,
            });
        }
    }
    Line { boxes, text, y }
}

/// The lines that start a paragraph: those after a gap much wider than the
/// page's usual one, and the first of each block after the first.
fn paragraph_breaks(lines: &[Line]) -> HashSet<usize> {
    let gaps: Vec<f32> = lines.windows(2).map(|pair| pair[1].y - pair[0].y).collect();
    let mut positive: Vec<f32> = gaps.iter().copied().filter(|gap| *gap > 0.0).collect();
    positive.sort_by(f32::total_cmp);
    let Some(median) = positive.get(positive.len() / 2).copied() else {
        return HashSet::new();
    };
    gaps.iter()
        .enumerate()
        .filter(|(_, gap)| **gap <= 0.0 || **gap > median * PARAGRAPH_GAP_MULTIPLIER)
        .map(|(at, _)| at + 1)
        .collect()
}

/// The size of the block's middle glyph, which every band is measured by.
fn median_height(glyphs: &[Glyph]) -> f32 {
    let mut heights: Vec<f32> = glyphs.iter().map(|glyph| glyph.height).collect();
    heights.sort_by(f32::total_cmp);
    heights.get(heights.len() / 2).copied().unwrap_or(1.0)
}

/// How many characters `glyphs` read as.
fn char_count(glyphs: &[Glyph]) -> usize {
    glyphs.iter().map(|glyph| glyph.text.chars().count()).sum()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// Glyphs for `text` set from `(x, y)` at size `size`, each character
    /// half the size wide and a space a gap of the same.
    pub(in super::super) fn words(text: &str, x: f32, y: f32, size: f32) -> Vec<Glyph> {
        let advance = size / 2.0;
        text.chars()
            .enumerate()
            .filter(|(_, ch)| *ch != ' ')
            .map(|(at, ch)| Glyph {
                height: size,
                text: ch.to_string(),
                width: advance,
                x: x + advance * at as f32,
                y,
            })
            .collect()
    }

    fn texts(page: &PageText) -> Vec<String> {
        page.lines
            .iter()
            .map(|line| line.text.iter().collect())
            .collect()
    }

    #[test]
    fn test_a_gap_as_wide_as_a_space_reads_as_one() {
        let page = PageText::new(words("two words", 10.0, 20.0, 10.0));
        assert_eq!(texts(&page), ["two words"]);
        let line = &page.lines[0];
        assert_eq!(line.boxes.len(), line.text.len(), "a box per character");
    }

    #[test]
    fn test_glyphs_drawn_out_of_order_read_left_to_right_top_to_bottom() {
        let mut glyphs = words("second", 10.0, 40.0, 10.0);
        glyphs.extend(words("first", 10.0, 20.0, 10.0));
        glyphs.reverse();
        assert_eq!(texts(&PageText::new(glyphs)), ["first", "second"]);
    }

    #[test]
    fn test_a_raised_glyph_stays_on_its_line() {
        // The E of the TeX logo sits lower than the T and X around it.
        let mut glyphs = words("T X", 10.0, 20.0, 10.0);
        glyphs.push(Glyph {
            height: 10.0,
            text: "E".to_string(),
            width: 5.0,
            x: 12.5,
            y: 22.0,
        });
        assert_eq!(PageText::new(glyphs).lines.len(), 1);
    }

    #[test]
    fn test_two_columns_read_down_one_and_then_the_other() {
        let mut glyphs = Vec::new();
        for row in 0..3 {
            let y = 20.0 + row as f32 * 14.0;
            glyphs.extend(words(&format!("left {row}"), 10.0, y, 10.0));
            glyphs.extend(words(&format!("right {row}"), 200.0, y, 10.0));
        }
        assert_eq!(
            texts(&PageText::new(glyphs)),
            ["left 0", "left 1", "left 2", "right 0", "right 1", "right 2"]
        );
    }

    #[test]
    fn test_margin_line_numbers_and_a_page_number_leave_two_columns() {
        // A review copy: numbers down both margins, and the page number
        // set under the gutter, too close to the text to be a row of its own.
        let mut glyphs = Vec::new();
        let rows = 60;
        for row in 0..rows {
            let y = 20.0 + row as f32 * 12.0;
            glyphs.extend(words(&format!("{:03}", row), 0.0, y, 10.0));
            glyphs.extend(words(&format!("left column line {row:02}"), 40.0, y, 10.0));
            glyphs.extend(words(&format!("right column line {row:02}"), 200.0, y, 10.0));
            glyphs.extend(words(&format!("{:03}", rows + row), 340.0, y, 10.0));
        }
        glyphs.extend(words("2", 180.0, 20.0 + rows as f32 * 12.0, 10.0));
        let texts = texts(&PageText::new(glyphs));
        let first_right = texts
            .iter()
            .position(|line| line.contains("right"))
            .unwrap();
        assert!(
            texts[..first_right].iter().all(|line| !line.contains("right")),
            "the left column is read through first: {texts:?}"
        );
        assert!(
            texts[first_right..].iter().all(|line| !line.contains("left")),
            "then the right one: {texts:?}"
        );
    }

    #[test]
    fn test_a_wide_gap_between_lines_starts_a_paragraph() {
        let mut glyphs = Vec::new();
        for (row, y) in [20.0, 34.0, 48.0, 72.0, 86.0].into_iter().enumerate() {
            glyphs.extend(words(&format!("line {row}"), 10.0, y, 10.0));
        }
        let page = PageText::new(glyphs);
        assert_eq!(
            page.lines.len(),
            5,
            "one block: no band across is wide enough"
        );
        assert_eq!(page.breaks, HashSet::from([3]));
    }

    #[test]
    fn test_a_ligature_is_split_into_its_letters() {
        let page = PageText::new(vec![Glyph {
            height: 10.0,
            text: "fi".to_string(),
            width: 6.0,
            x: 0.0,
            y: 10.0,
        }]);
        assert_eq!(texts(&page), ["fi"]);
        assert_eq!(page.lines[0].boxes[1].x, 3.0, "each letter half the glyph");
    }
}
