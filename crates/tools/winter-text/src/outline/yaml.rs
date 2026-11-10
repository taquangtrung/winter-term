//! A YAML document's outline: each key holding a block beneath it, and each
//! item of a sequence that is a mapping, shown by its first key and value,
//! nested as the indentation nests them. A key holding a plain value is a
//! setting, and is left out; so is the text a block scalar (`|`, `>`)
//! holds, which only looks like keys.

use winter_model::editor::JumpTarget;

use super::entry;

// ========================================================================
// Constants
// ========================================================================

/// What marks a sequence's item, and the item with nothing after it.
const ITEM: &str = "- ";
const BARE_ITEM: &str = "-";

/// What starts a new document in the stream.
const DOCUMENT_START: &str = "---";

// ========================================================================
// Data Structures
// ========================================================================

/// One line of content: how far it is indented, and its text after that.
struct Content<'a> {
    indent: usize,
    text: &'a str,
}

/// A mapping's key on a line, and what follows its colon.
struct Pair<'a> {
    key: &'a str,
    value: &'a str,
}

// ========================================================================
// Functions
// ========================================================================

/// The outline of the YAML document `lines`.
pub(super) fn outline(lines: &[String]) -> Vec<JumpTarget> {
    let mut outline = Vec::new();
    // How far each entry holding the line being read reaches in.
    let mut open: Vec<usize> = Vec::new();
    let mut row = 0;
    while row < lines.len() {
        let Some(line) = content(&lines[row]) else {
            row += 1;
            continue;
        };
        if line.text.starts_with(DOCUMENT_START) {
            open.clear();
            row += 1;
            continue;
        }
        let item = line
            .text
            .strip_prefix(ITEM)
            .or_else(|| (line.text == BARE_ITEM).then_some(""));
        let (body, reach) = match item {
            Some(rest) => (rest.trim_start(), line.indent + 1),
            None => (line.text, line.indent),
        };
        let pair = pair_of(body);
        let label = match (item, &pair) {
            // A mapping in a sequence, by its first key and value.
            (Some(_), Some(_)) => Some(body),
            (None, Some(pair)) if is_empty(pair.value) && holds_block(lines, row, line.indent) => {
                Some(pair.key)
            }
            _ => None,
        };
        if let Some(label) = label {
            while open.last().is_some_and(|holder| *holder >= reach) {
                open.pop();
            }
            outline.push(entry(row, open.len(), label));
            open.push(reach);
        }
        row += 1;
        // The text a block scalar holds: every line further in, or blank.
        if pair.is_some_and(|pair| pair.value.starts_with(['|', '>'])) {
            while row < lines.len()
                && content(&lines[row]).is_none_or(|next| next.indent > line.indent)
            {
                row += 1;
            }
        }
    }
    outline
}

/// `line`'s indent and text, where it holds more than a comment.
fn content(line: &str) -> Option<Content<'_>> {
    let text = line.trim_start_matches(' ');
    let text = text.trim_end();
    if text.is_empty() || text.starts_with('#') {
        return None;
    }
    Some(Content {
        indent: line.len() - line.trim_start_matches(' ').len(),
        text,
    })
}

/// The key `body` gives a value to, quoted or plain, and what follows its
/// colon.
fn pair_of(body: &str) -> Option<Pair<'_>> {
    let (key, rest) = match body.chars().next()? {
        quote @ ('"' | '\'') => {
            let close = body[1..].find(quote)? + 1;
            (&body[1..close], &body[close + 1..])
        }
        '{' | '[' | '?' => return None,
        _ => {
            let at = body.match_indices(':').find(|(at, _)| {
                body[at + 1..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace)
            })?;
            (body[..at.0].trim_end(), &body[at.0..])
        }
    };
    let value = rest.strip_prefix(':')?;
    (value.is_empty() || value.starts_with(char::is_whitespace)).then(|| Pair {
        key,
        value: value.trim(),
    })
}

/// Whether a key's `value` leaves its value to the lines below: nothing,
/// a comment, or an anchor or tag alone.
fn is_empty(value: &str) -> bool {
    value.is_empty()
        || value.starts_with('#')
        || (value.starts_with(['&', '!']) && !value.contains(char::is_whitespace))
}

/// Whether the key on line `row`, `indent` in, holds a block: the next
/// line of content is further in, or is a sequence's item as far in.
fn holds_block(lines: &[String], row: usize, indent: usize) -> bool {
    lines[row + 1..]
        .iter()
        .find_map(|line| content(line))
        .is_some_and(|next| {
            next.indent > indent
                || (next.indent == indent
                    && (next.text.starts_with(ITEM) || next.text == BARE_ITEM))
        })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::tests::{labels, lines};

    #[test]
    fn test_keys_holding_blocks_and_mapping_items_nest_and_settings_are_left_out() {
        let doc = lines(concat!(
            "name: ci\n",
            "on:\n",
            "  push:\n",
            "    branches: [main]\n",
            "jobs:\n",
            "  build:\n",
            "    # a comment\n",
            "    steps:\n",
            "    - uses: actions/checkout@v4\n",
            "    - name: Test\n",
            "      run: |\n",
            "        not: a key\n",
            "\n",
            "        echo done\n",
            "      env:\n",
            "        A: 1\n",
            "    - plain\n",
            "\"quoted key\":\n",
            "  - x\n",
        ));
        assert_eq!(
            labels(&outline(&doc)),
            [
                "    2  on",
                "    3    push",
                "    5  jobs",
                "    6    build",
                "    8      steps",
                "    9        uses: actions/checkout@v4",
                "   10        name: Test",
                "   15          env",
                "   18  quoted key",
            ]
        );
    }

    #[test]
    fn test_a_url_s_colon_or_an_inline_value_makes_no_key_hold_a_block() {
        let doc = lines("site: https://example.com\nflow: {a: 1}\nanchored: &base\n  b: 2\n");
        assert_eq!(labels(&outline(&doc)), ["    3  anchored"]);
    }
}
