//! The Vim text objects and the spans they cover: what `iw`, `a"`, `ip` and
//! their kin reach over, and where the bracket under `%` has its match. Pure
//! over whatever supplies the rows, so a grid and a file share one answer.

use super::words::char_class;

// ========================================================================
// Data Structures
// ========================================================================

/// An inclusive span between two points, each a row and a column within it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Span {
    /// The last point the span covers, taken with it.
    pub(crate) end: (usize, usize),
    /// The first point the span covers.
    pub(crate) start: (usize, usize),
}

// ========================================================================
// Traits
// ========================================================================

/// Rows of characters an object is looked for in. A terminal grid and a file
/// both answer this, and nothing here knows which one it is reading.
pub(crate) trait TextRows {
    /// How many rows there are to look through.
    fn row_count(&self) -> usize;

    /// Row `row` as characters, empty past the last row.
    fn row_chars(&self, row: usize) -> Vec<char>;
}

// ========================================================================
// Single-row objects
// ========================================================================

/// The inclusive column range a word object covers (`iw`/`aw`/`iW`/`aW`), or
/// `None` on an empty line. On blanks it takes the run of them, and `around`
/// adds the word after it, or the one before where nothing follows.
pub(crate) fn word_object(
    line: &[char],
    col: usize,
    big: bool,
    around: bool,
) -> Option<(usize, usize)> {
    if line.is_empty() {
        return None;
    }
    let col = col.min(line.len() - 1);
    let class = char_class(line[col], big);
    let mut start = col;
    let mut end = col;
    while start > 0 && char_class(line[start - 1], big) == class {
        start -= 1;
    }
    while end + 1 < line.len() && char_class(line[end + 1], big) == class {
        end += 1;
    }
    if around {
        // `aw` reaches to the blanks after the run, and where there are none
        // to the ones before it, so the pair of words does not close up.
        let want: fn(u8) -> bool = match class {
            0 => |c| c != 0,
            _ => |c| c == 0,
        };
        if end + 1 < line.len() && want(char_class(line[end + 1], big)) {
            let next = char_class(line[end + 1], big);
            end += 1;
            while end + 1 < line.len() && char_class(line[end + 1], big) == next {
                end += 1;
            }
        } else if start > 0 && want(char_class(line[start - 1], big)) {
            let prev = char_class(line[start - 1], big);
            start -= 1;
            while start > 0 && char_class(line[start - 1], big) == prev {
                start -= 1;
            }
        }
    }
    Some((start, end))
}

/// The inclusive column range a quoted run covers (`i"`/`a"`), or `None` when
/// the line holds no pair at or after `col`. An empty pair gives a range whose
/// start is past its end, which is the empty span between the quotes.
pub(crate) fn quote_object(
    line: &[char],
    col: usize,
    quote: char,
    around: bool,
) -> Option<(usize, usize)> {
    let mut quotes = Vec::new();
    let mut escaped = false;
    for (index, c) in line.iter().enumerate() {
        if *c == '\\' && !escaped {
            escaped = true;
            continue;
        }
        if *c == quote && !escaped {
            quotes.push(index);
        }
        escaped = false;
    }
    let (open, close) = quotes
        .chunks_exact(2)
        .map(|pair| (pair[0], pair[1]))
        .find(|(_, close)| col <= *close)?;
    if around {
        return Some((open, close));
    }
    match close > open + 1 {
        true => Some((open + 1, close - 1)),
        false => Some((open + 1, open)),
    }
}

/// The inclusive column range the sentence at `col` covers (`is`/`as`), or
/// `None` on an empty line. `as` takes the blanks after the sentence, or the
/// ones before it where none follow.
pub(crate) fn sentence_object(line: &[char], col: usize, around: bool) -> Option<(usize, usize)> {
    if line.is_empty() {
        return None;
    }
    let col = col.min(line.len() - 1);
    let (mut start, end, after) = sentence_spans(line)
        .into_iter()
        .find(|(start, _, after)| col >= *start && col < *after)?;
    // On the blanks between two sentences, the run of them is the object, the
    // way a run of blanks between words is what `iw` takes.
    if col >= end && !around {
        return Some((end, after.saturating_sub(1)));
    }
    if around && after == end {
        while start > 0 && line[start - 1].is_whitespace() {
            start -= 1;
        }
    }
    let last = if around { after } else { end };
    Some((start, last.max(start + 1) - 1))
}

// ========================================================================
// Multi-row objects
// ========================================================================

/// The span a bracket pair covers (`i(`/`a(` and their kin), searching back
/// for the opener the cursor is inside and on for the closer that answers it.
/// Nesting counts, so an inner pair is found before the one holding it.
pub(crate) fn bracket_object(
    rows: &impl TextRows,
    at: (usize, usize),
    open: char,
    close: char,
    around: bool,
) -> Option<Span> {
    let total = rows.row_count();
    let (mut row, col) = at;
    let mut col = col.min(rows.row_chars(row).len().saturating_sub(1));
    let mut depth = 0usize;
    let opener = loop {
        match rows.row_chars(row).get(col) {
            Some(c) if *c == close && (row, col) != at => depth += 1,
            Some(c) if *c == open => match depth {
                0 => break Some((row, col)),
                _ => depth -= 1,
            },
            _ => {}
        }
        if col > 0 {
            col -= 1;
        } else if row > 0 {
            row -= 1;
            col = rows.row_chars(row).len().saturating_sub(1);
        } else {
            break None;
        }
    }?;
    let (mut row, mut col) = opener;
    let mut depth = 0usize;
    let closer = loop {
        let line = rows.row_chars(row);
        match line.get(col) {
            Some(c) if *c == open => depth += 1,
            Some(c) if *c == close => match depth {
                0 | 1 => break Some((row, col)),
                _ => depth -= 1,
            },
            _ => {}
        }
        if col + 1 < line.len() {
            col += 1;
        } else if row + 1 < total {
            row += 1;
            col = 0;
        } else {
            break None;
        }
    }?;
    if around {
        return Some(Span {
            end: closer,
            start: opener,
        });
    }
    Some(Span {
        end: step_back(rows, closer),
        start: step_on(rows, opener, total),
    })
}

/// Where `%` goes from `at`: the match of the first bracket at or right of the
/// cursor on its row, forward from an opener and back from a closer, or `None`
/// when the row holds no bracket or nothing answers it.
pub(crate) fn matching_bracket(rows: &impl TextRows, at: (usize, usize)) -> Option<(usize, usize)> {
    let (row, col) = at;
    let line = rows.row_chars(row);
    let (found, bracket) = line
        .iter()
        .enumerate()
        .skip(col)
        .find_map(|(index, c)| pair_of(*c).map(|pair| (index, pair)))?;
    let (open, close, forward) = bracket;
    let total = rows.row_count();
    let mut depth = 0usize;
    let (mut row, mut col) = (row, found);
    loop {
        let line = rows.row_chars(row);
        match line.get(col) {
            Some(c) if *c == (if forward { open } else { close }) => depth += 1,
            Some(c) if *c == (if forward { close } else { open }) => match depth {
                0 | 1 => return Some((row, col)),
                _ => depth -= 1,
            },
            _ => {}
        }
        if forward {
            if col + 1 < line.len() {
                col += 1;
            } else if row + 1 < total {
                row += 1;
                col = 0;
            } else {
                return None;
            }
        } else if col > 0 {
            col -= 1;
        } else if row > 0 {
            row -= 1;
            col = rows.row_chars(row).len().saturating_sub(1);
        } else {
            return None;
        }
    }
}

/// The row a paragraph motion lands on (`{`/`}`): the next blank row past the
/// run the cursor is in, or the first or last row where there is none.
pub(crate) fn paragraph_edge(rows: &impl TextRows, row: usize, forward: bool) -> usize {
    let total = rows.row_count();
    if total == 0 {
        return 0;
    }
    let blank = |row: usize| rows.row_chars(row).iter().all(|c| c.is_whitespace());
    let step = |row: usize| match forward {
        true => (row + 1 < total).then(|| row + 1),
        false => row.checked_sub(1),
    };
    let mut at = match step(row) {
        Some(next) => next,
        None => return row,
    };
    // Off the blanks the cursor may be sitting among, then across the text,
    // which is what leaves the motion on the next boundary rather than on the
    // row beside the one it started on.
    while blank(at) {
        match step(at) {
            Some(next) => at = next,
            None => return at,
        }
    }
    while !blank(at) {
        match step(at) {
            Some(next) => at = next,
            None => return at,
        }
    }
    at
}

/// The inclusive row range a paragraph object covers (`ip`/`ap`): the run of
/// rows around `row` that are all blank or all not, and for `around` the run
/// that follows it, or the one before where nothing follows.
pub(crate) fn paragraph_object(
    rows: &impl TextRows,
    row: usize,
    around: bool,
) -> Option<(usize, usize)> {
    let total = rows.row_count();
    if row >= total {
        return None;
    }
    let blank = |row: usize| rows.row_chars(row).iter().all(|c| c.is_whitespace());
    let here = blank(row);
    let same = |candidate: usize| blank(candidate) == here;
    let mut start = row;
    let mut end = row;
    while start > 0 && same(start - 1) {
        start -= 1;
    }
    while end + 1 < total && same(end + 1) {
        end += 1;
    }
    if around {
        let mut after = end;
        while after + 1 < total && !same(after + 1) {
            after += 1;
        }
        if after > end {
            end = after;
        } else {
            while start > 0 && !same(start - 1) {
                start -= 1;
            }
        }
    }
    Some((start, end))
}

// ========================================================================
// Helpers
// ========================================================================

/// Every sentence of `line`, as `(start, end, after)`: where it begins, where
/// its text stops, and where the blanks following it stop. The tail of a row
/// that ends no sentence is one of its own, so a cursor anywhere on the row
/// lands in exactly one span.
fn sentence_spans(line: &[char]) -> Vec<(usize, usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    let mut at = 0;
    while at < line.len() {
        let Some(end) = sentence_end_at(line, at) else {
            at += 1;
            continue;
        };
        let mut after = end;
        while after < line.len() && line[after].is_whitespace() {
            after += 1;
        }
        spans.push((start, end, after));
        start = after;
        at = after.max(at + 1);
    }
    if start < line.len() {
        spans.push((start, line.len(), line.len()));
    }
    spans
}

/// Where the sentence ending at `at` stops: one past its `.`, `!` or `?` and
/// the closing quotes and brackets that follow, when a blank or the row's end
/// comes next. `None` where `at` ends no sentence, which is what keeps a
/// decimal point or a file extension from splitting one.
fn sentence_end_at(line: &[char], at: usize) -> Option<usize> {
    if !matches!(line[at], '.' | '!' | '?') {
        return None;
    }
    let mut end = at + 1;
    while end < line.len() && matches!(line[end], ')' | ']' | '"' | '\'') {
        end += 1;
    }
    match line.get(end) {
        None => Some(end),
        Some(c) if c.is_whitespace() => Some(end),
        Some(_) => None,
    }
}

/// The whole lines of the paragraph `row` sits in: the run of rows around it
/// that are all blank or all not, which over a terminal's output is one block
/// of output, one command's worth of it, or the gap between two.
///
/// `around` takes the blank rows that follow the run as well, and the ones
/// before it when none follow — vim's own `ap`, where `ip` stops at the
/// The pair a bracket character belongs to, and whether its match lies ahead
/// of it rather than behind.
fn pair_of(c: char) -> Option<(char, char, bool)> {
    Some(match c {
        '(' => ('(', ')', true),
        ')' => ('(', ')', false),
        '[' => ('[', ']', true),
        ']' => ('[', ']', false),
        '{' => ('{', '}', true),
        '}' => ('{', '}', false),
        _ => return None,
    })
}

/// One position on, over a row's end onto the next.
fn step_on(rows: &impl TextRows, at: (usize, usize), total: usize) -> (usize, usize) {
    let (row, col) = at;
    if col + 1 < rows.row_chars(row).len() {
        return (row, col + 1);
    }
    match row + 1 < total {
        true => (row + 1, 0),
        false => at,
    }
}

/// One position back, over a row's start onto the end of the one above.
fn step_back(rows: &impl TextRows, at: (usize, usize)) -> (usize, usize) {
    let (row, col) = at;
    if col > 0 {
        return (row, col - 1);
    }
    match row > 0 {
        true => (row - 1, rows.row_chars(row - 1).len().saturating_sub(1)),
        false => at,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines of text as the rows an object is looked for in.
    struct Lines(Vec<Vec<char>>);

    impl Lines {
        fn of(lines: &[&str]) -> Self {
            Self(lines.iter().map(|line| line.chars().collect()).collect())
        }
    }

    impl TextRows for Lines {
        fn row_count(&self) -> usize {
            self.0.len()
        }

        fn row_chars(&self, row: usize) -> Vec<char> {
            self.0.get(row).cloned().unwrap_or_default()
        }
    }

    #[test]
    fn test_a_word_object_takes_the_run_and_around_takes_the_blanks_after_it() {
        let line: Vec<char> = "one two  three".chars().collect();
        assert_eq!(word_object(&line, 5, false, false), Some((4, 6)), "`iw`");
        assert_eq!(word_object(&line, 5, false, true), Some((4, 8)), "`aw`");
        // On the blanks between two words, `iw` is the run of blanks alone.
        assert_eq!(word_object(&line, 7, false, false), Some((7, 8)));
        // With nothing after the last word, `aw` reaches back instead.
        assert_eq!(word_object(&line, 10, false, true), Some((7, 13)));
    }

    #[test]
    fn test_a_quote_object_finds_the_pair_the_cursor_is_in_or_ahead_of() {
        let line: Vec<char> = "say \"hi\" now".chars().collect();
        assert_eq!(quote_object(&line, 6, '"', false), Some((5, 6)), "`i\"`");
        assert_eq!(quote_object(&line, 6, '"', true), Some((4, 7)), "`a\"`");
        // From before the pair, the one ahead is the one meant.
        assert_eq!(quote_object(&line, 0, '"', false), Some((5, 6)));
        // An escaped quote is not one of the pair.
        let escaped: Vec<char> = "\"a\\\"b\"".chars().collect();
        assert_eq!(quote_object(&escaped, 1, '"', true), Some((0, 5)));
    }

    #[test]
    fn test_a_bracket_object_crosses_lines_and_counts_nesting() {
        let rows = Lines::of(&["fn call(a, b) {", "    inner(x)", "}"]);
        assert_eq!(
            bracket_object(&rows, (1, 6), '{', '}', false),
            Some(Span {
                end: (1, 11),
                start: (1, 0)
            }),
            "the lines between the braces"
        );
        assert_eq!(
            bracket_object(&rows, (0, 12), '(', ')', false),
            Some(Span {
                end: (0, 11),
                start: (0, 8)
            }),
            "from the closing bracket itself, that pair and not the one outside"
        );
        assert_eq!(
            bracket_object(&rows, (0, 9), '(', ')', true),
            Some(Span {
                end: (0, 12),
                start: (0, 7)
            }),
            "the nearer pair, not the outer one"
        );
    }

    #[test]
    fn test_the_bracket_match_runs_both_ways_and_over_line_ends() {
        let rows = Lines::of(&["if (a) {", "    go()", "}"]);
        assert_eq!(matching_bracket(&rows, (0, 3)), Some((0, 5)), "on to `)`");
        assert_eq!(matching_bracket(&rows, (0, 5)), Some((0, 3)), "back to `(`");
        assert_eq!(
            matching_bracket(&rows, (0, 7)),
            Some((2, 0)),
            "down to the closing brace"
        );
        assert_eq!(matching_bracket(&rows, (2, 0)), Some((0, 7)), "and back up");
        // The first bracket at or right of the cursor is the one matched.
        assert_eq!(matching_bracket(&rows, (1, 0)), Some((1, 7)));
        // Past every bracket on the row there is nothing to match.
        assert_eq!(matching_bracket(&rows, (0, 8)), None);
    }

    #[test]
    fn test_a_paragraph_motion_stops_on_the_blank_row_past_the_text() {
        let rows = Lines::of(&["one", "two", "", "", "three", "four"]);
        assert_eq!(paragraph_edge(&rows, 0, true), 2, "the blank below");
        assert_eq!(paragraph_edge(&rows, 2, true), 5, "over the blanks");
        assert_eq!(paragraph_edge(&rows, 5, false), 3, "the blank above");
        assert_eq!(paragraph_edge(&rows, 0, false), 0, "nothing above the top");
    }

    #[test]
    fn test_a_paragraph_object_is_the_run_of_rows_and_around_adds_the_blanks() {
        let rows = Lines::of(&["one", "two", "", "three"]);
        assert_eq!(paragraph_object(&rows, 0, false), Some((0, 1)), "`ip`");
        assert_eq!(paragraph_object(&rows, 0, true), Some((0, 2)), "`ap`");
        assert_eq!(paragraph_object(&rows, 2, false), Some((2, 2)), "the blank");
    }
}
