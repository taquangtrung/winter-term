//! Balanced delimiter and S-expression navigation and structural editing:
//! jumping between delimiters (`next`, `prev`, `up`, `down`) and structural
//! transformations (`slurp`, `barf`, `splice`, `raise`).
//!
//! Works across parentheses, brackets, and braces in all languages.

use winter_model::textedit::buffer::TextBuffer;

// ========================================================================
// Data Structures
// ========================================================================

/// An enclosing bracket pair and its positions in character indices.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EnclosingPair {
    /// Opening bracket character.
    pub open_char: char,
    /// Offset of opening bracket.
    pub open_idx: usize,
    /// Closing bracket character.
    pub close_char: char,
    /// Offset of closing bracket.
    pub close_idx: usize,
}

// ========================================================================
// Position conversions
// ========================================================================

/// Convert `(row, col)` into a character offset in `lines.join("\n")`.
pub fn offset_of(lines: &[String], row: usize, col: usize) -> usize {
    let mut off = 0;
    for (r, line) in lines.iter().enumerate() {
        let count = line.chars().count();
        if r == row {
            return off + col.min(count);
        }
        off += count + 1; // newline
    }
    off
}

/// Convert a character offset in `lines.join("\n")` back into `(row, col)`.
pub fn pos_at(lines: &[String], mut offset: usize) -> (usize, usize) {
    if lines.is_empty() {
        return (0, 0);
    }
    for (r, line) in lines.iter().enumerate() {
        let count = line.chars().count();
        if offset <= count {
            return (r, offset);
        }
        // Account for newline
        if offset == count + 1 && r + 1 == lines.len() {
            return (r, count);
        }
        offset = offset.saturating_sub(count + 1);
    }
    let last_row = lines.len() - 1;
    (last_row, lines[last_row].chars().count())
}

// ========================================================================
// Bracket Matching & Delimiters
// ========================================================================

fn is_open_delim(c: char) -> bool {
    matches!(c, '(' | '[' | '{')
}

fn is_close_delim(c: char) -> bool {
    matches!(c, ')' | ']' | '}')
}

fn matching_close(open: char) -> char {
    match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        _ => open,
    }
}

fn matching_open(close: char) -> char {
    match close {
        ')' => '(',
        ']' => '[',
        '}' => '{',
        _ => close,
    }
}

/// Find the innermost enclosing bracket pair surrounding `offset`.
pub fn find_enclosing_pair(chars: &[char], offset: usize) -> Option<EnclosingPair> {
    let offset = offset.min(chars.len());
    let mut stack = Vec::new();
    let mut best: Option<EnclosingPair> = None;

    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if is_open_delim(c) {
            stack.push((c, i));
        } else if is_close_delim(c) {
            if let Some((open_c, open_idx)) = stack.pop() {
                if matching_close(open_c) == c {
                    // Check if offset is enclosed: open_idx <= offset <= i
                    if open_idx <= offset && offset <= i {
                        // Innermost pair has maximum open_idx
                        if best.is_none_or(|b| open_idx > b.open_idx) {
                            best = Some(EnclosingPair {
                                open_char: open_c,
                                open_idx,
                                close_char: c,
                                close_idx: i,
                            });
                        }
                    }
                }
            }
        }
        i += 1;
    }

    best
}

/// Locate the S-expression span (start..end character offset) starting at `at`.
pub fn sexp_span_forward(chars: &[char], at: usize) -> Option<(usize, usize)> {
    let mut start = at;
    while start < chars.len() && chars[start].is_whitespace() {
        start += 1;
    }
    if start >= chars.len() {
        return None;
    }

    let c = chars[start];
    if is_open_delim(c) {
        let close = matching_close(c);
        let mut depth = 0;
        let mut i = start;
        while i < chars.len() {
            if chars[i] == c {
                depth += 1;
            } else if chars[i] == close {
                depth -= 1;
                if depth == 0 {
                    return Some((start, i + 1));
                }
            }
            i += 1;
        }
        Some((start, chars.len()))
    } else if c == '"' || c == '\'' || c == '`' {
        let quote = c;
        let mut i = start + 1;
        let mut escaped = false;
        while i < chars.len() {
            if chars[i] == '\\' && !escaped {
                escaped = true;
            } else if chars[i] == quote && !escaped {
                return Some((start, i + 1));
            } else {
                escaped = false;
            }
            i += 1;
        }
        Some((start, chars.len()))
    } else {
        // Run of non-whitespace, non-delimiter characters
        let mut end = start;
        while end < chars.len()
            && !chars[end].is_whitespace()
            && !is_open_delim(chars[end])
            && !is_close_delim(chars[end])
            && chars[end] != '"'
            && chars[end] != '\''
            && chars[end] != '`'
        {
            end += 1;
        }
        if end == start {
            end = (start + 1).min(chars.len());
        }
        Some((start, end))
    }
}

/// Locate the previous S-expression span (start..end character offset) ending before `at`.
pub fn sexp_span_backward(chars: &[char], at: usize) -> Option<(usize, usize)> {
    if at == 0 {
        return None;
    }
    let mut end = at.min(chars.len());
    while end > 0 && chars[end - 1].is_whitespace() {
        end -= 1;
    }
    if end == 0 {
        return None;
    }

    let last_char = chars[end - 1];
    if is_close_delim(last_char) {
        let open = matching_open(last_char);
        let mut depth = 0;
        let mut i = end;
        while i > 0 {
            i -= 1;
            if chars[i] == last_char {
                depth += 1;
            } else if chars[i] == open {
                depth -= 1;
                if depth == 0 {
                    return Some((i, end));
                }
            }
        }
        Some((0, end))
    } else if last_char == '"' || last_char == '\'' || last_char == '`' {
        let quote = last_char;
        let mut i = end.saturating_sub(1);
        while i > 0 {
            i -= 1;
            if chars[i] == quote && (i == 0 || chars[i - 1] != '\\') {
                return Some((i, end));
            }
        }
        Some((0, end))
    } else {
        let mut start = end;
        while start > 0
            && !chars[start - 1].is_whitespace()
            && !is_open_delim(chars[start - 1])
            && !is_close_delim(chars[start - 1])
            && chars[start - 1] != '"'
            && chars[start - 1] != '\''
            && chars[start - 1] != '`'
        {
            start -= 1;
        }
        Some((start, end))
    }
}

// ========================================================================
// Buffer operations
// ========================================================================

/// Slurp forward: pulls the next sibling S-expression into the current enclosing bracket pair.
/// Example: `(a b) c` -> `(a b c)`
pub fn slurp_forward(buffer: &mut TextBuffer) -> bool {
    let lines = buffer.lines().to_vec();
    let text = lines.join("\n");
    let chars: Vec<char> = text.chars().collect();
    let cursor_off = offset_of(&lines, buffer.row(), buffer.col());

    let Some(pair) = find_enclosing_pair(&chars, cursor_off) else {
        return false;
    };

    // Find next sexp after close_idx
    let Some((next_start, next_end)) = sexp_span_forward(&chars, pair.close_idx + 1) else {
        return false;
    };

    // Construct new text:
    // chars[0..pair.close_idx] + " " + chars[next_start..next_end] + pair.close_char + chars[next_end..]
    let before_close: String = chars[..pair.close_idx].iter().collect();
    let slurped: String = chars[next_start..next_end].iter().collect();
    let after_next: String = chars[next_end..].iter().collect();

    // Preserve space between content and slurped item if needed
    let sep =
        if before_close.ends_with(char::is_whitespace) || before_close.ends_with(pair.open_char) {
            ""
        } else {
            " "
        };

    let new_text = format!(
        "{before_close}{sep}{slurped}{}{after_next}",
        pair.close_char
    );
    apply_text_change(buffer, new_text, cursor_off);
    true
}

/// Slurp backward: pulls the previous sibling S-expression into current enclosing brackets.
/// Example: `a (b c)` -> `(a b c)`
pub fn slurp_backward(buffer: &mut TextBuffer) -> bool {
    let lines = buffer.lines().to_vec();
    let text = lines.join("\n");
    let chars: Vec<char> = text.chars().collect();
    let cursor_off = offset_of(&lines, buffer.row(), buffer.col());

    let Some(pair) = find_enclosing_pair(&chars, cursor_off) else {
        return false;
    };

    let Some((prev_start, prev_end)) = sexp_span_backward(&chars, pair.open_idx) else {
        return false;
    };

    let before_prev: String = chars[..prev_start].iter().collect();
    let slurped: String = chars[prev_start..prev_end].iter().collect();
    let after_open: String = chars[pair.open_idx + 1..].iter().collect();

    let sep =
        if after_open.starts_with(char::is_whitespace) || after_open.starts_with(pair.close_char) {
            ""
        } else {
            " "
        };

    let new_text = format!("{before_prev}{}{slurped}{sep}{after_open}", pair.open_char);
    apply_text_change(buffer, new_text, cursor_off);
    true
}

/// Barf forward: pushes the last child S-expression out of current enclosing brackets.
/// Example: `(a b c)` -> `(a b) c`
pub fn barf_forward(buffer: &mut TextBuffer) -> bool {
    let lines = buffer.lines().to_vec();
    let text = lines.join("\n");
    let chars: Vec<char> = text.chars().collect();
    let cursor_off = offset_of(&lines, buffer.row(), buffer.col());

    let Some(pair) = find_enclosing_pair(&chars, cursor_off) else {
        return false;
    };

    let Some((last_start, last_end)) = sexp_span_backward(&chars, pair.close_idx) else {
        return false;
    };

    if last_start <= pair.open_idx {
        return false;
    }

    // Trim trailing whitespace before last_start inside the brackets
    let mut trim_before = last_start;
    while trim_before > pair.open_idx + 1 && chars[trim_before - 1].is_whitespace() {
        trim_before -= 1;
    }

    let before_barf: String = chars[..trim_before].iter().collect();
    let barfed: String = chars[last_start..last_end].iter().collect();
    let after_close: String = chars[pair.close_idx + 1..].iter().collect();

    let new_text = format!("{before_barf}{} {barfed}{after_close}", pair.close_char);
    apply_text_change(buffer, new_text, cursor_off);
    true
}

/// Barf backward: pushes the first child S-expression out of current enclosing brackets.
/// Example: `(a b c)` -> `a (b c)`
pub fn barf_backward(buffer: &mut TextBuffer) -> bool {
    let lines = buffer.lines().to_vec();
    let text = lines.join("\n");
    let chars: Vec<char> = text.chars().collect();
    let cursor_off = offset_of(&lines, buffer.row(), buffer.col());

    let Some(pair) = find_enclosing_pair(&chars, cursor_off) else {
        return false;
    };

    let Some((first_start, first_end)) = sexp_span_forward(&chars, pair.open_idx + 1) else {
        return false;
    };

    if first_end >= pair.close_idx {
        return false;
    }

    let mut trim_after = first_end;
    while trim_after < pair.close_idx && chars[trim_after].is_whitespace() {
        trim_after += 1;
    }

    let before_open: String = chars[..pair.open_idx].iter().collect();
    let barfed: String = chars[first_start..first_end].iter().collect();
    let inside_rest: String = chars[trim_after..].iter().collect();

    let new_text = format!("{before_open}{barfed} {}{inside_rest}", pair.open_char);
    apply_text_change(buffer, new_text, cursor_off);
    true
}

/// Splice: unwraps the current enclosing bracket pair.
/// Example: `(a b c)` -> `a b c`
pub fn splice(buffer: &mut TextBuffer) -> bool {
    let lines = buffer.lines().to_vec();
    let text = lines.join("\n");
    let chars: Vec<char> = text.chars().collect();
    let cursor_off = offset_of(&lines, buffer.row(), buffer.col());

    let Some(pair) = find_enclosing_pair(&chars, cursor_off) else {
        return false;
    };

    let before_open: String = chars[..pair.open_idx].iter().collect();
    let inner: String = chars[pair.open_idx + 1..pair.close_idx].iter().collect();
    let after_close: String = chars[pair.close_idx + 1..].iter().collect();

    let new_text = format!("{before_open}{inner}{after_close}");
    apply_text_change(buffer, new_text, cursor_off.saturating_sub(1));
    true
}

/// Find an enclosing bracket pair strictly outside `span_start..span_end`.
pub fn find_parent_enclosing_pair(
    chars: &[char],
    span_start: usize,
    span_end: usize,
) -> Option<EnclosingPair> {
    let mut stack = Vec::new();
    let mut best: Option<EnclosingPair> = None;

    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if is_open_delim(c) {
            stack.push((c, i));
        } else if is_close_delim(c) {
            if let Some((open_c, open_idx)) = stack.pop() {
                let encloses = open_idx < span_start && span_end <= i;
                if matching_close(open_c) == c
                    && encloses
                    && best.is_none_or(|b| open_idx > b.open_idx)
                {
                    best = Some(EnclosingPair {
                        open_char: open_c,
                        open_idx,
                        close_char: c,
                        close_idx: i,
                    });
                }
            }
        }
        i += 1;
    }

    best
}

/// Raise: replaces the enclosing bracket pair with only the sexp under cursor.
/// Example: `(a (b c) d)` with cursor on `(b c)` -> `(b c)`
pub fn raise(buffer: &mut TextBuffer) -> bool {
    let lines = buffer.lines().to_vec();
    let text = lines.join("\n");
    let chars: Vec<char> = text.chars().collect();
    let cursor_off = offset_of(&lines, buffer.row(), buffer.col());

    // First find the current sexp at/surrounding the cursor
    let target_span =
        sexp_span_forward(&chars, cursor_off).or_else(|| sexp_span_backward(&chars, cursor_off));

    let Some((s_start, s_end)) = target_span else {
        return false;
    };

    // Find the enclosing pair outside this sexp
    let Some(parent) = find_parent_enclosing_pair(&chars, s_start, s_end) else {
        return false;
    };

    let before_open: String = chars[..parent.open_idx].iter().collect();
    let raised: String = chars[s_start..s_end].iter().collect();
    let after_close: String = chars[parent.close_idx + 1..].iter().collect();

    let new_text = format!("{before_open}{raised}{after_close}");
    apply_text_change(buffer, new_text, parent.open_idx);
    true
}

// ========================================================================
// Navigation
// ========================================================================

/// Move to the start of the next S-expression.
pub fn move_next_sexp(buffer: &mut TextBuffer) -> bool {
    let lines = buffer.lines().to_vec();
    let text = lines.join("\n");
    let chars: Vec<char> = text.chars().collect();
    let cursor_off = offset_of(&lines, buffer.row(), buffer.col());

    // Advance past current sexp or whitespace
    let Some((_, end)) = sexp_span_forward(&chars, cursor_off) else {
        return false;
    };

    let target = if end <= cursor_off {
        let Some((next_start, _)) = sexp_span_forward(&chars, cursor_off + 1) else {
            return false;
        };
        next_start
    } else {
        end
    };

    let (r, c) = pos_at(&lines, target);
    buffer.move_to(r, c);
    true
}

/// Move to the start of the previous S-expression.
pub fn move_prev_sexp(buffer: &mut TextBuffer) -> bool {
    let lines = buffer.lines().to_vec();
    let text = lines.join("\n");
    let chars: Vec<char> = text.chars().collect();
    let cursor_off = offset_of(&lines, buffer.row(), buffer.col());

    let Some((start, _)) = sexp_span_backward(&chars, cursor_off) else {
        return false;
    };

    let (r, c) = pos_at(&lines, start);
    buffer.move_to(r, c);
    true
}

/// Jump upward/out of enclosing bracket pair (backward to open bracket, or forward to close).
pub fn move_up_sexp(buffer: &mut TextBuffer, forward: bool) -> bool {
    let lines = buffer.lines().to_vec();
    let text = lines.join("\n");
    let chars: Vec<char> = text.chars().collect();
    let cursor_off = offset_of(&lines, buffer.row(), buffer.col());

    let Some(mut pair) = find_enclosing_pair(&chars, cursor_off) else {
        return false;
    };

    // Already on the delimiter it would move to: the pair around this one.
    let on_target = cursor_off
        == if forward {
            pair.close_idx
        } else {
            pair.open_idx
        };
    if on_target {
        if let Some(parent) = find_parent_enclosing_pair(&chars, pair.open_idx, pair.close_idx) {
            pair = parent;
        }
    }

    let target = if forward {
        pair.close_idx
    } else {
        pair.open_idx
    };

    let (r, c) = pos_at(&lines, target);
    buffer.move_to(r, c);
    true
}

/// Jump downward into inner child bracket pair.
pub fn move_down_sexp(buffer: &mut TextBuffer, forward: bool) -> bool {
    let lines = buffer.lines().to_vec();
    let text = lines.join("\n");
    let chars: Vec<char> = text.chars().collect();
    let cursor_off = offset_of(&lines, buffer.row(), buffer.col());

    let target = if forward {
        chars[cursor_off..]
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, c)| is_open_delim(**c))
            .map(|(idx, _)| cursor_off + idx)
    } else {
        chars[..cursor_off]
            .iter()
            .enumerate()
            .rfind(|(_, c)| is_close_delim(**c))
            .map(|(idx, _)| idx)
    };

    let Some(target_idx) = target else {
        return false;
    };

    let (r, c) = pos_at(&lines, target_idx);
    buffer.move_to(r, c);
    true
}

// ========================================================================
// Helpers
// ========================================================================

fn apply_text_change(buffer: &mut TextBuffer, new_text: String, landing_offset: usize) {
    let old_line_count = buffer.lines().len();
    let new_lines: Vec<String> = new_text.split('\n').map(String::from).collect();
    let (land_row, land_col) = pos_at(&new_lines, landing_offset);
    buffer.replace_rows(0, old_line_count, new_lines);
    buffer.move_to(land_row, land_col);
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_enclosing_pair() {
        let text = "(foo [bar {baz}])";
        let chars: Vec<char> = text.chars().collect();
        let pair = find_enclosing_pair(&chars, 11).expect("inner pair");
        assert_eq!(pair.open_char, '{');
        assert_eq!(pair.close_char, '}');

        let mid = find_enclosing_pair(&chars, 6).expect("middle pair");
        assert_eq!(mid.open_char, '[');
        assert_eq!(mid.close_char, ']');

        let outer = find_enclosing_pair(&chars, 2).expect("outer pair");
        assert_eq!(outer.open_char, '(');
        assert_eq!(outer.close_char, ')');
    }

    #[test]
    fn test_slurp_forward() {
        let mut buf = TextBuffer::new(vec!["(a b) c".to_string()]);
        buf.move_to(0, 2);
        assert!(slurp_forward(&mut buf));
        assert_eq!(buf.lines(), &["(a b c)"]);
    }

    #[test]
    fn test_slurp_backward() {
        let mut buf = TextBuffer::new(vec!["a (b c)".to_string()]);
        buf.move_to(0, 4);
        assert!(slurp_backward(&mut buf));
        assert_eq!(buf.lines(), &["(a b c)"]);
    }

    #[test]
    fn test_barf_forward() {
        let mut buf = TextBuffer::new(vec!["(a b c)".to_string()]);
        buf.move_to(0, 2);
        assert!(barf_forward(&mut buf));
        assert_eq!(buf.lines(), &["(a b) c"]);
    }

    #[test]
    fn test_barf_backward() {
        let mut buf = TextBuffer::new(vec!["(a b c)".to_string()]);
        buf.move_to(0, 4);
        assert!(barf_backward(&mut buf));
        assert_eq!(buf.lines(), &["a (b c)"]);
    }

    #[test]
    fn test_splice() {
        let mut buf = TextBuffer::new(vec!["(a b c)".to_string()]);
        buf.move_to(0, 2);
        assert!(splice(&mut buf));
        assert_eq!(buf.lines(), &["a b c"]);
    }

    #[test]
    fn test_raise() {
        let mut buf = TextBuffer::new(vec!["(a (b c) d)".to_string()]);
        buf.move_to(0, 3);
        assert!(raise(&mut buf));
        assert_eq!(buf.lines(), &["(b c)"]);
    }

    #[test]
    fn test_sexp_navigation() {
        let mut buf = TextBuffer::new(vec!["(foo (bar baz))".to_string()]);
        buf.move_to(0, 0);
        // Down into inner
        assert!(move_down_sexp(&mut buf, true));
        assert_eq!(buf.col(), 5); // on '(' of (bar baz)

        // Up to outer
        assert!(move_up_sexp(&mut buf, false));
        assert_eq!(buf.col(), 0);

        // Up to closing
        assert!(move_up_sexp(&mut buf, true));
        assert_eq!(buf.col(), 14);

        // Move prev and next
        buf.move_to(0, 1);
        assert!(move_next_sexp(&mut buf));
        assert_eq!(buf.col(), 4);

        assert!(move_prev_sexp(&mut buf));
        assert_eq!(buf.col(), 1);
    }
}
