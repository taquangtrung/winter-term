//! Commenting lines out and back in (`C-/`), with whatever the file's
//! language writes comments with.

// ========================================================================
// Data Structures
// ========================================================================

/// How a language comments a line out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Marker {
    /// A comment that runs to the end of the line, opened by this.
    Line(&'static str),
    /// A comment wrapped around the line, opened and closed by these.
    Block(&'static str, &'static str),
}

// ========================================================================
// Functions
// ========================================================================

/// `lines` commented out, or back in when every one of them that holds
/// anything already is. Blank lines are left alone either way, and the marker
/// goes in at the shallowest indent of the lines it covers, so a commented
/// block keeps its shape.
pub(super) fn toggle(lines: &[String], marker: Marker) -> Vec<String> {
    let filled: Vec<&String> = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if filled.is_empty() {
        return lines.to_vec();
    }
    if filled.iter().all(|line| is_commented(line, marker)) {
        return lines.iter().map(|line| uncommented(line, marker)).collect();
    }
    let indent = filled
        .iter()
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|line| match line.trim().is_empty() {
            true => line.clone(),
            false => commented(line, indent, marker),
        })
        .collect()
}

fn is_commented(line: &str, marker: Marker) -> bool {
    let text = line.trim();
    match marker {
        Marker::Line(open) => text.starts_with(open),
        Marker::Block(open, close) => text.starts_with(open) && text.ends_with(close),
    }
}

/// `line` with the marker put in at byte `indent`, which the caller found in
/// the leading blanks of every line covered.
fn commented(line: &str, indent: usize, marker: Marker) -> String {
    let (lead, text) = line.split_at(indent);
    match marker {
        Marker::Line(open) => format!("{lead}{open} {text}"),
        Marker::Block(open, close) => format!("{lead}{open} {text} {close}"),
    }
}

/// `line` with its marker taken out, and the one space it was written with.
fn uncommented(line: &str, marker: Marker) -> String {
    let text = line.trim_start();
    let lead = &line[..line.len() - text.len()];
    let inner = match marker {
        Marker::Line(open) => text
            .strip_prefix(open)
            .map(|rest| rest.strip_prefix(' ').unwrap_or(rest)),
        Marker::Block(open, close) => text
            .trim_end()
            .strip_prefix(open)
            .and_then(|rest| rest.strip_suffix(close))
            .map(|rest| {
                let rest = rest.strip_prefix(' ').unwrap_or(rest);
                rest.strip_suffix(' ').unwrap_or(rest)
            }),
    };
    match inner {
        Some(inner) => format!("{lead}{inner}"),
        None => line.to_string(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn test_lines_comment_at_their_shallowest_indent_and_come_back_unchanged() {
        let code = lines(&["    if x {", "", "        y();", "    }"]);
        let out = toggle(&code, Marker::Line("//"));
        assert_eq!(
            out,
            lines(&["    // if x {", "", "    //     y();", "    // }"])
        );
        assert_eq!(toggle(&out, Marker::Line("//")), code, "and back again");
    }

    #[test]
    fn test_a_mix_of_commented_and_plain_lines_comments_them_all() {
        let mixed = lines(&["# done", "todo"]);
        assert_eq!(
            toggle(&mixed, Marker::Line("#")),
            lines(&["# # done", "# todo"])
        );
    }

    #[test]
    fn test_a_language_with_only_block_comments_wraps_each_line() {
        let html = lines(&["<p>hi</p>"]);
        let out = toggle(&html, Marker::Block("<!--", "-->"));
        assert_eq!(out, lines(&["<!-- <p>hi</p> -->"]));
        assert_eq!(toggle(&out, Marker::Block("<!--", "-->")), html);
    }
}
