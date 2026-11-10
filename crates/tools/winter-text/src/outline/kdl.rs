//! A KDL document's outline: each node that opens a block, nested as the
//! blocks nest; a node with no block of its own is a setting, not a part
//! of the document's shape, and is left out.

use winter_model::editor::JumpTarget;

use super::entry;

// ========================================================================
// Functions
// ========================================================================

/// A KDL document's blocks: each node that opens one, indented by how many
/// hold it. A block commented out with `/-` is left out, with all it holds.
pub(super) fn outline(lines: &[String]) -> Vec<JumpTarget> {
    let mut outline = Vec::new();
    let mut depth = 0usize;
    let mut quoted = false;
    let mut comments = 0usize;
    // The depth a block commented out opened at, while inside it.
    let mut dropped: Option<usize> = None;
    for (row, line) in lines.iter().enumerate() {
        let start = depth;
        let mut opened = false;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if comments > 0 {
                match (c, chars.peek()) {
                    ('*', Some('/')) => {
                        chars.next();
                        comments -= 1;
                    }
                    ('/', Some('*')) => {
                        chars.next();
                        comments += 1;
                    }
                    _ => {}
                }
                continue;
            }
            if quoted {
                match c {
                    '\\' => {
                        chars.next();
                    }
                    '"' => quoted = false,
                    _ => {}
                }
                continue;
            }
            match (c, chars.peek()) {
                ('/', Some('/')) => break,
                ('/', Some('*')) => {
                    chars.next();
                    comments += 1;
                }
                ('"', _) => quoted = true,
                ('{', _) => {
                    opened |= depth == start;
                    depth += 1;
                }
                ('}', _) => {
                    depth = depth.saturating_sub(1);
                    if dropped == Some(depth) {
                        dropped = None;
                    }
                }
                _ => {}
            }
        }
        if !opened || dropped.is_some() {
            continue;
        }
        let text = line.trim();
        match text.starts_with("/-") {
            true if depth > start => dropped = Some(start),
            true => {}
            false => outline.push(entry(row, start, text)),
        }
    }
    outline
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::tests::{labels, lines};

    #[test]
    fn test_kdl_blocks_nest_and_braces_in_strings_or_comments_do_not_count() {
        let doc = lines(concat!(
            "theme \"dark\"\n",
            "lsp {\n",
            "    auto-start \"{rust}\" // {\n",
            "    server \"ra\" { command \"ra\"; }\n",
            "    server \"x\" {\n",
            "        /* { */ install \"x\"\n",
            "    }\n",
            "}\n",
            "/-compile {\n",
            "    dock {\n",
            "    }\n",
            "}\n",
            "cursor {\n",
            "}",
        ));
        assert_eq!(
            labels(&outline(&doc)),
            [
                "    2  lsp {",
                "    4    server \"ra\" { command \"ra\"; }",
                "    5    server \"x\" {",
                "   13  cursor {",
            ]
        );
    }
}
