//! Writing Markdown, as magic-vscode and Emacs' markdown-mode help with it:
//! moving between headings, promoting and demoting a heading or a list item
//! (alone or with everything under it), wrapping a word in emphasis or code,
//! ticking a task, numbering a list again, starting the next item, and
//! counting the words.
//!
//! Everything here reads and returns lines, and never the editor, so it is
//! tested on text alone.

// ========================================================================
// Constants
// ========================================================================

/// The deepest heading Markdown has.
const MAX_LEVEL: usize = 6;

/// What a list item is indented by for each level it is demoted.
const LIST_INDENT: &str = "  ";

/// The markers that open and close a fenced code block.
const FENCES: [&str; 2] = ["```", "~~~"];

/// A task's box, ticked and not.
const TASK_OPEN: &str = "[ ] ";
const TASK_DONE: &str = "[x] ";

// ========================================================================
// Data Structures
// ========================================================================

/// A heading: the row it is on and how deep it is (`#` is 1).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Heading {
    pub(super) level: usize,
    pub(super) row: usize,
}

/// Where a list item's text starts: its indent, its marker (`-`, `*`, `+`,
/// or a number with `.` or `)`), and the space after the marker.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ListItem {
    indent: String,
    marker: String,
    number: Option<u64>,
    /// The byte the item's own text starts at, past the marker's space.
    text_at: usize,
}

// ========================================================================
// Headings
// ========================================================================

/// Every heading in `lines`, outside fenced code, which can hold a `#` line
/// that is no heading at all.
pub(super) fn headings(lines: &[String]) -> Vec<Heading> {
    let mut fenced = false;
    let mut found = Vec::new();
    for (row, line) in lines.iter().enumerate() {
        if FENCES.iter().any(|fence| line.trim_start().starts_with(fence)) {
            fenced = !fenced;
            continue;
        }
        if let Some(level) = heading_level(line).filter(|_| !fenced) {
            found.push(Heading { level, row });
        }
    }
    found
}

/// How deep the heading on `line` is, for an ATX heading: one to six `#`s,
/// then a space or nothing.
pub(super) fn heading_level(line: &str) -> Option<usize> {
    let marks = line.chars().take_while(|c| *c == '#').count();
    let rest = &line[marks..];
    ((1..=MAX_LEVEL).contains(&marks) && (rest.is_empty() || rest.starts_with(' ')))
        .then_some(marks)
}

/// The heading after `row`, or before it, that `wanted` keeps; `None` at
/// either end.
pub(super) fn step_heading(
    lines: &[String],
    row: usize,
    forward: bool,
    wanted: impl Fn(&Heading) -> bool,
) -> Option<usize> {
    let found = headings(lines);
    let mut candidates = found.iter().filter(|heading| wanted(heading));
    let hit = match forward {
        true => candidates.find(|heading| heading.row > row),
        false => candidates.rev().find(|heading| heading.row < row),
    };
    hit.map(|heading| heading.row)
}

/// The heading whose section holds `row`: the last one at or above it.
pub(super) fn heading_over(lines: &[String], row: usize) -> Option<Heading> {
    headings(lines).into_iter().rev().find(|heading| heading.row <= row)
}

/// The heading over the section `row` is in, one level up: the parent.
pub(super) fn parent_heading(lines: &[String], row: usize) -> Option<usize> {
    let own = heading_over(lines, row)?;
    headings(lines)
        .into_iter()
        .rev()
        .find(|heading| heading.row < own.row && heading.level < own.level)
        .map(|heading| heading.row)
}

/// The rows a heading's subtree covers: itself and everything down to the
/// next heading as high as it or higher.
pub(super) fn subtree_end(lines: &[String], heading: Heading) -> usize {
    headings(lines)
        .into_iter()
        .find(|next| next.row > heading.row && next.level <= heading.level)
        .map_or(lines.len(), |next| next.row)
}

// ========================================================================
// Promoting and demoting
// ========================================================================

/// `line` promoted (`deeper` unset) or demoted: a heading takes one `#`
/// fewer or more, within one to six; a list item moves out or in by one
/// indent. `None` for a line that is neither, or one already as far as it
/// goes.
pub(super) fn shifted(line: &str, deeper: bool) -> Option<String> {
    if let Some(level) = heading_level(line) {
        let level = match deeper {
            true => (level < MAX_LEVEL).then_some(level + 1)?,
            false => (level > 1).then_some(level - 1)?,
        };
        let rest = line.trim_start_matches('#');
        return Some(format!("{}{rest}", "#".repeat(level)));
    }
    list_item(line)?;
    match deeper {
        true => Some(format!("{LIST_INDENT}{line}")),
        false => line.strip_prefix(LIST_INDENT).map(str::to_string),
    }
}

/// Every heading of `lines` shifted as [`shifted`] does, and the rest kept,
/// or `None` where a heading cannot go that way: a subtree moves as one or
/// not at all, so its shape stays.
pub(super) fn shifted_subtree(lines: &[String], deeper: bool) -> Option<Vec<String>> {
    let mut out = Vec::with_capacity(lines.len());
    let mut fenced = false;
    for line in lines {
        if FENCES.iter().any(|fence| line.trim_start().starts_with(fence)) {
            fenced = !fenced;
        }
        match heading_level(line).filter(|_| !fenced) {
            Some(_) => out.push(shifted(line, deeper)?),
            None => out.push(line.clone()),
        }
    }
    Some(out)
}

// ========================================================================
// Lists
// ========================================================================

/// The list item `line` is, if it is one.
fn list_item(line: &str) -> Option<ListItem> {
    let indent_len = line.len() - line.trim_start().len();
    let rest = &line[indent_len..];
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    let (marker, number) = match digits {
        0 => {
            let first = rest.chars().next()?;
            (matches!(first, '-' | '*' | '+').then(|| first.to_string())?, None)
        }
        _ => {
            let close = rest[digits..].chars().next()?;
            if !matches!(close, '.' | ')') {
                return None;
            }
            (rest[..=digits].to_string(), rest[..digits].parse().ok())
        }
    };
    let after = &rest[marker.len()..];
    if !(after.is_empty() || after.starts_with(' ')) {
        return None;
    }
    Some(ListItem {
        indent: line[..indent_len].to_string(),
        text_at: (indent_len + marker.len() + 1).min(line.len()),
        marker,
        number,
    })
}

/// `line` with its task box ticked or cleared, or given one if it is a list
/// item without; `None` for a line that is no list item.
pub(super) fn toggled_task(line: &str) -> Option<String> {
    let item = list_item(line)?;
    let (head, text) = line.split_at(item.text_at);
    let text = match (text.strip_prefix(TASK_OPEN), text.strip_prefix(TASK_DONE)) {
        (Some(rest), _) => format!("{TASK_DONE}{rest}"),
        (_, Some(rest)) => format!("{TASK_OPEN}{rest}"),
        _ => format!("{TASK_OPEN}{text}"),
    };
    let head = match head.ends_with(' ') {
        true => head.to_string(),
        false => format!("{head} "),
    };
    Some(format!("{head}{text}"))
}

/// What the next item after `line` starts with: the same indent and marker,
/// the number one on, and an empty box after a task. `None` for a line that
/// is no list item.
pub(super) fn next_item_prefix(line: &str) -> Option<String> {
    let item = list_item(line)?;
    let marker = match item.number {
        Some(number) => {
            let close = item.marker.chars().last().unwrap_or('.');
            format!("{}{close}", number + 1)
        }
        None => item.marker.clone(),
    };
    let text = &line[item.text_at..];
    let task = match text.starts_with(TASK_OPEN) || text.starts_with(TASK_DONE) {
        true => TASK_OPEN,
        false => "",
    };
    Some(format!("{}{marker} {task}", item.indent))
}

/// The ordered list `row` is in, numbered again from its first item's
/// number: its rows and their new text. Items deeper in, and the lines that
/// carry an item on, are left as they are. `None` where `row` is in no
/// ordered list.
pub(super) fn renumbered(lines: &[String], row: usize) -> Option<(usize, Vec<String>)> {
    let own = list_item(&lines[row]).filter(|item| item.number.is_some())?;
    // An item of the list, or a line indented past its items, which carries
    // the item above it on.
    let belongs = |line: &String| {
        !line.trim().is_empty()
            && (line.len() - line.trim_start().len() > own.indent.len()
                || list_item(line)
                    .is_some_and(|item| item.indent == own.indent && item.number.is_some()))
    };
    let first = (0..=row).rev().take_while(|at| belongs(&lines[*at])).last()?;
    let last = (row..lines.len()).take_while(|at| belongs(&lines[*at])).last()?;
    let mut number = list_item(&lines[first])
        .and_then(|item| item.number.filter(|_| item.indent == own.indent))
        .unwrap_or(1);
    let mut out = Vec::with_capacity(last - first + 1);
    for line in &lines[first..=last] {
        match list_item(line).filter(|item| item.indent == own.indent && item.number.is_some()) {
            Some(item) => {
                let close = item.marker.chars().last().unwrap_or('.');
                out.push(format!(
                    "{}{number}{close}{}",
                    item.indent,
                    &line[item.indent.len() + item.marker.len()..]
                ));
                number += 1;
            }
            None => out.push(line.clone()),
        }
    }
    Some((first, out))
}

// ========================================================================
// Emphasis
// ========================================================================

/// `line` with the characters `start..end` wrapped in `mark` on both sides,
/// or unwrapped where `mark` already wraps them, and where the same
/// characters now are. Counts are in characters, not bytes.
pub(super) fn toggled_wrap(line: &str, start: usize, end: usize, mark: &str) -> (String, usize, usize) {
    let chars: Vec<char> = line.chars().collect();
    let mark_chars: Vec<char> = mark.chars().collect();
    let width = mark_chars.len();
    let around = start >= width
        && chars[start - width..start] == mark_chars[..]
        && chars.get(end..end + width) == Some(&mark_chars[..]);
    let inside = end - start >= 2 * width
        && chars[start..start + width] == mark_chars[..]
        && chars[end - width..end] == mark_chars[..];
    let text = |parts: &[&[char]]| parts.iter().flat_map(|part| part.iter()).collect::<String>();
    match (around, inside) {
        (true, _) => (
            text(&[&chars[..start - width], &chars[start..end], &chars[end + width..]]),
            start - width,
            end - width,
        ),
        (_, true) => (
            text(&[&chars[..start], &chars[start + width..end - width], &chars[end..]]),
            start,
            end - 2 * width,
        ),
        _ => (
            text(&[&chars[..start], &mark_chars, &chars[start..end], &mark_chars, &chars[end..]]),
            start + width,
            end + width,
        ),
    }
}

/// The characters of the word `col` is on in `line`, as `start..end`, or
/// `None` off any word.
pub(super) fn word_at(line: &str, col: usize) -> Option<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let is_word = |c: &char| c.is_alphanumeric() || *c == '_' || *c == '-' || *c == '\'';
    if !chars.get(col).is_some_and(is_word) {
        return None;
    }
    let start = (0..=col).rev().take_while(|at| is_word(&chars[*at])).last()?;
    let end = (col..chars.len()).take_while(|at| is_word(&chars[*at])).last()? + 1;
    Some((start, end))
}

// ========================================================================
// Counting
// ========================================================================

/// How many words `lines` hold, outside fenced code, front matter, and the
/// markup itself.
pub(super) fn word_count(lines: &[String]) -> usize {
    let mut fenced = false;
    let mut front = lines.first().is_some_and(|line| line.trim() == "---");
    let mut count = 0;
    for (row, line) in lines.iter().enumerate() {
        if front {
            front = !(row > 0 && line.trim() == "---");
            continue;
        }
        if FENCES.iter().any(|fence| line.trim_start().starts_with(fence)) {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        count += line
            .split_whitespace()
            .filter(|word| word.chars().any(char::is_alphanumeric))
            .count();
    }
    count
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    const DOC: &str = "# A\ntext\n## B\n```\n# not a heading\n```\n### C\n## D\n# E";

    #[test]
    fn test_headings_are_found_outside_code_and_stepped_between() {
        let doc = lines(DOC);
        let rows: Vec<usize> = headings(&doc).iter().map(|h| h.row).collect();
        assert_eq!(rows, [0, 2, 6, 7, 8]);
        assert_eq!(step_heading(&doc, 2, true, |_| true), Some(6));
        assert_eq!(step_heading(&doc, 2, true, |h| h.level == 2), Some(7));
        assert_eq!(step_heading(&doc, 7, false, |h| h.level == 2), Some(2));
        assert_eq!(step_heading(&doc, 8, true, |_| true), None);
        assert_eq!(parent_heading(&doc, 6), Some(2));
        assert_eq!(parent_heading(&doc, 4), Some(0), "a line under ## B");
        assert_eq!(heading_level("#hashtag"), None);
    }

    #[test]
    fn test_a_heading_or_an_item_is_promoted_and_demoted_within_bounds() {
        assert_eq!(shifted("## B", true).as_deref(), Some("### B"));
        assert_eq!(shifted("## B", false).as_deref(), Some("# B"));
        assert_eq!(shifted("# B", false), None, "no shallower heading");
        assert_eq!(shifted("###### F", true), None, "no deeper heading");
        assert_eq!(shifted("- item", true).as_deref(), Some("  - item"));
        assert_eq!(shifted("  1. item", false).as_deref(), Some("1. item"));
        assert_eq!(shifted("plain", true), None);
        let doc = lines(DOC);
        let tree = shifted_subtree(&doc[2..subtree_end(&doc, Heading { level: 2, row: 2 })], true);
        assert_eq!(
            tree.expect("shifts"),
            lines("### B\n```\n# not a heading\n```\n#### C")
        );
    }

    #[test]
    fn test_a_task_box_is_added_ticked_and_cleared() {
        assert_eq!(toggled_task("- buy").as_deref(), Some("- [ ] buy"));
        assert_eq!(toggled_task("- [ ] buy").as_deref(), Some("- [x] buy"));
        assert_eq!(toggled_task("  2. [x] buy").as_deref(), Some("  2. [ ] buy"));
        assert_eq!(toggled_task("-"), Some("- [ ] ".to_string()));
        assert_eq!(toggled_task("text"), None);
    }

    #[test]
    fn test_the_next_item_carries_the_marker_on() {
        assert_eq!(next_item_prefix("  - a").as_deref(), Some("  - "));
        assert_eq!(next_item_prefix("9) a").as_deref(), Some("10) "));
        assert_eq!(next_item_prefix("- [x] done").as_deref(), Some("- [ ] "));
        assert_eq!(next_item_prefix("-not a list"), None);
    }

    #[test]
    fn test_an_ordered_list_is_numbered_again_from_its_first_item() {
        let doc = lines("intro\n3. a\n7. b\n   more of b\n   - nested\n1. c\n\nafter");
        let (first, rows) = renumbered(&doc, 5).expect("in a list");
        assert_eq!(first, 1);
        assert_eq!(rows, lines("3. a\n4. b\n   more of b\n   - nested\n5. c"));
        assert_eq!(renumbered(&doc, 0), None);
    }

    #[test]
    fn test_a_word_is_wrapped_and_unwrapped_in_a_mark() {
        let (line, start, end) = toggled_wrap("say hi now", 4, 6, "**");
        assert_eq!((line.as_str(), start, end), ("say **hi** now", 6, 8));
        let (line, start, end) = toggled_wrap(&line, start, end, "**");
        assert_eq!((line.as_str(), start, end), ("say hi now", 4, 6), "around: unwrapped");
        let (line, _, _) = toggled_wrap("say `hi` now", 4, 8, "`");
        assert_eq!(line, "say hi now", "selected with the marks: unwrapped");
        assert_eq!(word_at("say hi-there now", 5), Some((4, 12)));
        assert_eq!(word_at("say  now", 3), None);
    }

    #[test]
    fn test_words_are_counted_outside_code_front_matter_and_markup() {
        let doc = lines("---\ntitle: x\n---\n# Two words\n- one\n```\nnot counted\n```\n**bold** text");
        assert_eq!(word_count(&doc), 5);
    }
}
