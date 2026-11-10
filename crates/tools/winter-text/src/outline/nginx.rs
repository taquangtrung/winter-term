//! An nginx configuration's outline: each directive that opens a block
//! (`http`, `server`, `location /api/`, `upstream`), nested as the blocks
//! nest. A directive ending in `;` is a setting, and is left out.

use winter_model::editor::JumpTarget;

use super::entry;

// ========================================================================
// Functions
// ========================================================================

/// The outline of the nginx configuration `lines`.
pub(super) fn outline(lines: &[String]) -> Vec<JumpTarget> {
    let mut outline = Vec::new();
    let mut depth = 0usize;
    // The quote a string open across lines began with.
    let mut quoted: Option<char> = None;
    for (row, line) in lines.iter().enumerate() {
        let start = depth;
        let mut opened = false;
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            if let Some(quote) = quoted {
                match c {
                    '\\' => {
                        chars.next();
                    }
                    _ if c == quote => quoted = None,
                    _ => {}
                }
                continue;
            }
            match c {
                '#' => break,
                '"' | '\'' => quoted = Some(c),
                '{' => {
                    opened |= depth == start;
                    depth += 1;
                }
                '}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        if opened {
            outline.push(entry(row, start, line.trim()));
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
    fn test_blocks_nest_and_braces_in_strings_or_comments_do_not_count() {
        let doc = lines(concat!(
            "worker_processes 4;\n",
            "http {\n",
            "    # server {\n",
            "    server {\n",
            "        return 200 \"{ok}\";\n",
            "        location /api/ {\n",
            "            proxy_pass http://app;\n",
            "        }\n",
            "    }\n",
            "}\n",
        ));
        assert_eq!(
            labels(&outline(&doc)),
            [
                "    2  http {",
                "    4    server {",
                "    6      location /api/ {",
            ]
        );
    }
}
