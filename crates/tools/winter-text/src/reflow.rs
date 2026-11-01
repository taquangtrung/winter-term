//! Paragraph reflow, hard-wrapping, and smart paragraph selection.
//!
//! Mirrors `magic.vim.toggleWrapParagraph` and `magic.vim.smartSelectParagraph`:
//! toggles a paragraph between hard-wrapped at 80 columns and joined into a single line,
//! preserving comment markers, Markdown lists, and indentation.

use winter_model::textedit::buffer::TextBuffer;
use winter_model::vim::objects::paragraph_object;

// ========================================================================
// Constants
// ========================================================================

/// Default target column width for paragraph reflow.
pub const DEFAULT_WRAP_WIDTH: usize = 80;

// ========================================================================
// Functions
// ========================================================================

/// Inspect a line's leading prefix: comment marker (`// `, `# `, etc.),
/// Markdown marker (`- `, `* `, `1. `, `> `), or whitespace indentation.
/// Returns `(prefix, continuation_prefix, content)`.
pub fn classify_line(line: &str) -> (&str, String, &str) {
    let trimmed = line.trim_start();
    let indent_len = line.len() - trimmed.len();
    let indent = &line[..indent_len];

    // Check for common comment markers
    for marker in &["/// ", "// ", "## ", "# ", ";; ", "; ", "-- "] {
        if trimmed.starts_with(marker) {
            let lead_len = indent_len + marker.len();
            let prefix = &line[..lead_len];
            let continuation_prefix = prefix.to_string();
            let content = &line[lead_len..];
            return (prefix, continuation_prefix, content);
        }
    }

    // Check for Markdown list bullet or blockquote
    for bullet in &["- ", "* ", "+ ", "> "] {
        if trimmed.starts_with(bullet) {
            let lead_len = indent_len + bullet.len();
            let prefix = &line[..lead_len];
            // Continuation aligns under the content
            let continuation_prefix = " ".repeat(lead_len);
            let content = &line[lead_len..];
            return (prefix, continuation_prefix, content);
        }
    }

    // Plain indentation
    let prefix = indent;
    let continuation_prefix = indent.to_string();
    let content = trimmed;
    (prefix, continuation_prefix, content)
}

/// Reflow or join the paragraph at `row` in `buffer`.
/// Returns true if the buffer was modified.
pub fn toggle_wrap(buffer: &mut TextBuffer, wrap_width: usize) -> bool {
    let row = buffer.row();
    let Some((start, end)) = paragraph_object(buffer, row, false) else {
        return false;
    };

    let lines = buffer.lines().to_vec();
    if start > end || end >= lines.len() {
        return false;
    }

    let is_multiline = start < end;
    if is_multiline {
        // Multi-line paragraph -> join into a single line
        let first_line = &lines[start];
        let (prefix, _, first_content) = classify_line(first_line);

        let mut words: Vec<String> = first_content.split_whitespace().map(String::from).collect();

        for line in &lines[start + 1..=end] {
            let (_, _, content) = classify_line(line);
            words.extend(content.split_whitespace().map(String::from));
        }

        let joined = format!("{prefix}{}", words.join(" "));
        buffer.replace_rows(start, end - start + 1, vec![joined]);
        buffer.move_to(start, prefix.chars().count());
        true
    } else {
        // Single-line paragraph -> reflow to wrap_width
        let line = &lines[start];
        let (prefix, cont_prefix, content) = classify_line(line);
        let words: Vec<&str> = content.split_whitespace().collect();
        if words.is_empty() {
            return false;
        }

        let mut new_lines = Vec::new();
        let mut cur_line = prefix.to_string();
        let mut cur_len = prefix.chars().count();
        let mut first = true;

        for word in words {
            let word_len = word.chars().count();
            if !first && cur_len + 1 + word_len > wrap_width {
                new_lines.push(cur_line);
                cur_line = cont_prefix.clone();
                cur_len = cont_prefix.chars().count();
                first = true;
            }

            if first {
                cur_line.push_str(word);
                cur_len += word_len;
                first = false;
            } else {
                cur_line.push(' ');
                cur_line.push_str(word);
                cur_len += 1 + word_len;
            }
        }
        if !cur_line.is_empty() {
            new_lines.push(cur_line);
        }

        if new_lines.is_empty() {
            return false;
        }

        let count = new_lines.len();
        buffer.replace_rows(start, 1, new_lines);
        buffer.move_to(start.min(buffer.lines().len() - 1), 0);
        let _ = count;
        true
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toggle_wrap_single_line_to_multiline() {
        let long_line = "    This is a long sentence that should definitely be wrapped across multiple lines when the target wrap width is set to forty characters.";
        let mut buf = TextBuffer::new(vec![long_line.to_string()]);
        assert!(toggle_wrap(&mut buf, 40));
        assert!(buf.lines().len() > 1);
        for line in buf.lines() {
            assert!(line.starts_with("    "), "indentation preserved: {line}");
        }

        // Now toggle back: should join into 1 line
        assert!(toggle_wrap(&mut buf, 40));
        assert_eq!(buf.lines().len(), 1);
        assert_eq!(buf.lines()[0], long_line);
    }

    #[test]
    fn test_toggle_wrap_comment() {
        let comment = "// One two three four five six seven eight nine ten eleven twelve.";
        let mut buf = TextBuffer::new(vec![comment.to_string()]);
        assert!(toggle_wrap(&mut buf, 30));
        assert!(buf.lines().len() > 1);
        for line in buf.lines() {
            assert!(line.starts_with("// "), "comment marker preserved: {line}");
        }

        assert!(toggle_wrap(&mut buf, 30));
        assert_eq!(buf.lines().len(), 1);
        assert_eq!(buf.lines()[0], comment);
    }
}
