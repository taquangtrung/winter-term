//! An HTML page's outline: its headings, `<h1>` to `<h6>`, each indented
//! below the shallowest, named by its text with any tags inside taken out,
//! as the page's own outline is. What `<script>`, `<style>`, and comments
//! hold is passed over.

use winter_model::editor::JumpTarget;

use super::entry;

// ========================================================================
// Constants
// ========================================================================

/// The elements whose text is code, not the page's, by their start tags,
/// and what ends each.
const CODE_ELEMENTS: [(&str, &str); 2] = [("<script", "</script"), ("<style", "</style")];

/// What opens and closes a comment.
const COMMENT: (&str, &str) = ("<!--", "-->");

// ========================================================================
// Data Structures
// ========================================================================

/// A heading being read: where it starts, its level, and its text so far.
struct Heading {
    level: usize,
    row: usize,
    text: String,
}

// ========================================================================
// Functions
// ========================================================================

/// The outline of the HTML page `lines`.
pub(super) fn outline(lines: &[String]) -> Vec<JumpTarget> {
    let mut found: Vec<Heading> = Vec::new();
    let mut heading: Option<Heading> = None;
    // The end of a comment or a code element, while inside one.
    let mut skipping: Option<&str> = None;
    for (row, line) in lines.iter().enumerate() {
        // Tags are found in a lowercased copy, whose offsets are the line's.
        let lower = line.to_ascii_lowercase();
        let mut at = 0;
        while at < line.len() {
            if let Some(end) = skipping {
                match lower[at..].find(end) {
                    Some(found) => {
                        at += found + end.len();
                        skipping = None;
                    }
                    None => at = line.len(),
                }
                continue;
            }
            if let Some(open) = heading.as_mut() {
                let close = format!("</h{}", open.level);
                match lower[at..].find(&close) {
                    Some(end) => {
                        open.text.push_str(&line[at..at + end]);
                        found.extend(heading.take());
                        at += end + close.len();
                    }
                    None => {
                        open.text.push_str(&line[at..]);
                        open.text.push(' ');
                        at = line.len();
                    }
                }
                continue;
            }
            let Some(next) = lower[at..].find('<') else {
                break;
            };
            at += next;
            let rest = &lower[at..];
            if rest.starts_with(COMMENT.0) {
                skipping = Some(COMMENT.1);
                at += COMMENT.0.len();
                continue;
            }
            if let Some((start, end)) = CODE_ELEMENTS.iter().find(|(start, _)| is_tag(rest, start))
            {
                skipping = Some(end);
                at += start.len();
                continue;
            }
            match heading_level(rest) {
                Some(level) => {
                    let Some(close) = rest.find('>') else {
                        break;
                    };
                    heading = Some(Heading {
                        level,
                        row,
                        text: String::new(),
                    });
                    at += close + 1;
                }
                None => at += 1,
            }
        }
    }
    let top = found.iter().map(|heading| heading.level).min().unwrap_or(1);
    found
        .into_iter()
        .map(|heading| entry(heading.row, heading.level - top, &text_of(&heading.text)))
        .collect()
}

/// Whether `rest` starts with the tag `start` (`<script`), not with a
/// longer tag beginning the same.
fn is_tag(rest: &str, start: &str) -> bool {
    rest.strip_prefix(start)
        .and_then(|after| after.chars().next())
        .is_some_and(|c| c == '>' || c.is_whitespace())
}

/// The level of the heading whose start tag `rest` starts with.
fn heading_level(rest: &str) -> Option<usize> {
    let mut chars = rest.strip_prefix("<h")?.chars();
    let level = chars
        .next()?
        .to_digit(10)
        .filter(|level| (1..=6).contains(level))?;
    chars
        .next()
        .filter(|c| *c == '>' || c.is_whitespace())
        .map(|_| level as usize)
}

/// A heading's text as read: the tags inside it taken out, its spaces
/// made single.
fn text_of(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::tests::{labels, lines};

    #[test]
    fn test_headings_nest_by_level_and_code_and_comments_are_passed_over() {
        let doc = lines(concat!(
            "<header><H2 class=\"t\">Getting <em>started</em></H2></header>\n",
            "<!-- <h2>Hidden</h2> -->\n",
            "<script>let s = \"<h2>no</h2>\";</script>\n",
            "<h3>Install\n",
            "  steps</h3><hr>\n",
            "<h2>Usage</h2>\n",
        ));
        assert_eq!(
            labels(&outline(&doc)),
            [
                "    1  Getting started",
                "    4    Install steps",
                "    6  Usage",
            ]
        );
    }
}
