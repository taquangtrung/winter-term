//! Typing by context, as magic-vscode's edit commands type: `RET` carries a
//! line comment or a block comment's `*` on to the next line, and in a
//! Markdown list goes on under the item's text (an empty item's bullet
//! moving down a line instead); `TAB` and `S-TAB` on a Markdown heading or
//! a bullet indent and outdent it, the bullet turning `-`, `+`, `*` by
//! depth; and `S-DEL` blanks a bullet out.

use crate::model::editor::EditorOutcome;
use crate::model::input::{Key, KeyCode, Mods};
use crate::model::textedit::modal::EditMode;

use super::comment::{self, Marker};
use super::writing::Prose;
use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// The bullets a Markdown list cycles through as an item is indented.
const BULLETS: [char; 3] = ['-', '+', '*'];

// ========================================================================
// Data Structures
// ========================================================================

/// A Markdown list item's opening: its indent, its marker (`-`, `1.`), and
/// the space after it, task box included.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Item {
    indent: String,
    marker: String,
    gap: String,
}

// ========================================================================
// TextEditor: typing by context
// ========================================================================

impl TextEditor {
    /// A key while typing that the context gives a meaning of its own, or
    /// `None` for one typed as it always is.
    pub(super) fn context_key(&mut self, key: &Key) -> Option<EditorOutcome> {
        if self.modal.mode() != EditMode::Insert || self.modal.is_read_only() {
            return None;
        }
        let done = match key.chord() {
            (Mods::NONE, KeyCode::Enter) => self.smart_newline(),
            (Mods::NONE, KeyCode::Tab) => self.indent_by_context(true),
            (Mods::SHIFT, KeyCode::Tab) => self.indent_by_context(false),
            (Mods::SHIFT, KeyCode::Backspace) => self.blank_bullet(),
            _ => false,
        };
        done.then_some(EditorOutcome::Consumed)
    }

    /// `RET` by context. Says whether it did anything but a plain new line.
    fn smart_newline(&mut self) -> bool {
        let row = self.buffer().row();
        let line: Vec<char> = self.line_of(row).chars().collect();
        let col = self.buffer().col().min(line.len());
        let before: String = line[..col].iter().collect();
        let after: String = line[col..].iter().collect();
        let next = match self.prose() {
            Some(Prose::Markdown) => match list_item(&before) {
                // An item with nothing in it: the bullet moves down a line,
                // leaving this one blank.
                Some(item) if line.iter().collect::<String>().trim() == item.marker.trim() => {
                    let opening = format!("{}{}{}", item.indent, item.marker, item.gap);
                    self.buffer_mut().replace_rows(row, 1, vec![String::new(), opening.clone()]);
                    self.buffer_mut().move_to_typing(row + 1, opening.chars().count());
                    return true;
                }
                Some(item) => {
                    let width = item.marker.chars().count() + item.gap.chars().count();
                    Some(format!("{}{}", item.indent, " ".repeat(width)))
                }
                None => None,
            },
            _ => self.comment_continuation(&line.iter().collect::<String>()),
        };
        let Some(next) = next else {
            return false;
        };
        let at = next.chars().count();
        self.buffer_mut().replace_rows(row, 1, vec![before, format!("{next}{after}")]);
        self.buffer_mut().move_to_typing(row + 1, at);
        true
    }

    /// What the line after `line` opens with to go on with its comment:
    /// the same line comment, a block comment's ` * `, or after a block
    /// comment's close, the indent it stood at.
    fn comment_continuation(&self, line: &str) -> Option<String> {
        let marker = self.doc().grammar.and_then(|grammar| comment::marker_of(grammar.name()))?;
        let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
        let body = &line[indent.len()..];
        let Marker::Line(opener) = marker else {
            return None;
        };
        if opener == "//" {
            if body.starts_with("*/") {
                return Some(indent.chars().skip(1).collect());
            }
            if body.starts_with("/*") {
                return Some(format!("{indent} * "));
            }
            if body.starts_with('*') {
                let star: String = body.chars().take_while(|c| *c == '*' || c.is_whitespace()).collect();
                return Some(format!("{indent}{star}"));
            }
        }
        if !body.starts_with(opener) {
            return None;
        }
        // The whole run of the opener (`///`, `##`) and the space after it.
        let lead: String = body
            .chars()
            .take_while(|c| opener.contains(*c))
            .chain(body.chars().skip_while(|c| opener.contains(*c)).take_while(|c| c.is_whitespace()))
            .collect();
        Some(format!("{indent}{lead}"))
    }

    /// `TAB` (`indent`) or `S-TAB` on a Markdown heading or a bullet: one
    /// level deeper or shallower. Says whether it did.
    fn indent_by_context(&mut self, indent: bool) -> bool {
        if self.prose() != Some(Prose::Markdown) {
            return false;
        }
        let row = self.buffer().row();
        let line = self.line_of(row);
        let col = self.buffer().col();
        if let Some(hashes) = heading_level(&line) {
            let changed = match indent {
                true => format!("#{line}"),
                false if hashes > 1 => line[1..].to_string(),
                false => return true,
            };
            let shift: isize = if indent { 1 } else { -1 };
            self.buffer_mut().replace_rows(row, 1, vec![changed]);
            self.buffer_mut().move_to_typing(row, (col as isize + shift).max(0) as usize);
            return true;
        }
        let Some(item) = list_item(&line).filter(|item| BULLETS.iter().any(|b| item.marker.starts_with(*b))) else {
            return false;
        };
        let opening = item.indent.chars().count() + item.marker.chars().count() + item.gap.chars().count();
        if indent && col > opening {
            // Past the bullet, a tab is a tab.
            return false;
        }
        let bullet = item.marker.chars().next().unwrap_or('-');
        let at = BULLETS.iter().position(|b| *b == bullet).unwrap_or(0);
        let width = item.marker.chars().count() + item.gap.chars().count();
        let rest: String = line.chars().skip(opening).collect();
        let (new_indent, new_bullet) = match indent {
            true => (format!("{}{}", item.indent, " ".repeat(width)), BULLETS[(at + 1) % BULLETS.len()]),
            false if item.indent.chars().count() >= width => (
                item.indent.chars().skip(width).collect(),
                BULLETS[(at + BULLETS.len() - 1) % BULLETS.len()],
            ),
            false => return true,
        };
        let changed = format!("{new_indent}{new_bullet}{}{rest}", item.gap);
        let moved = new_indent.chars().count() as isize - item.indent.chars().count() as isize;
        self.buffer_mut().replace_rows(row, 1, vec![changed]);
        self.buffer_mut().move_to_typing(row, (col as isize + moved).max(0) as usize);
        true
    }

    /// `S-DEL` just past a bullet: the bullet goes, its room kept, so the
    /// text reads on as a paragraph of the item above. Says whether it did.
    fn blank_bullet(&mut self) -> bool {
        if self.prose() != Some(Prose::Markdown) {
            return false;
        }
        let row = self.buffer().row();
        let line = self.line_of(row);
        let col = self.buffer().col();
        let before: String = line.chars().take(col).collect();
        let Some(item) = list_item(&before) else {
            return false;
        };
        let opening = item.indent.chars().count() + item.marker.chars().count() + item.gap.chars().count();
        if col != opening {
            return false;
        }
        let rest: String = line.chars().skip(opening).collect();
        let changed = format!("{}{}{}{rest}", item.indent, " ".repeat(item.marker.chars().count()), item.gap);
        self.buffer_mut().replace_rows(row, 1, vec![changed]);
        self.buffer_mut().move_to_typing(row, col);
        true
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The list item `text` opens, where it opens one: a bullet or a number,
/// then a space, and a task box after it.
fn list_item(text: &str) -> Option<Item> {
    let indent: String = text.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    let rest = &text[indent.len()..];
    let marker_len = match rest.chars().next()? {
        '-' | '+' | '*' => 1,
        c if c.is_ascii_digit() => {
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            match rest[digits..].chars().next() {
                Some('.' | ')') => digits + 1,
                _ => return None,
            }
        }
        _ => return None,
    };
    let after = &rest[marker_len..];
    let spaces: String = after.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    if spaces.is_empty() {
        return None;
    }
    let tail = &after[spaces.len()..];
    let task = ["[ ] ", "[x] ", "[X] "].into_iter().find(|task| tail.starts_with(task)).unwrap_or("");
    Some(Item {
        indent,
        marker: rest[..marker_len].to_string(),
        gap: format!("{spaces}{task}"),
    })
}

/// How many `#` open `line` as a Markdown heading.
fn heading_level(line: &str) -> Option<usize> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    (hashes > 0 && line[hashes..].starts_with([' ', '\t'])).then_some(hashes)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_list_item_is_read_with_its_number_or_bullet_and_task_box() {
        let item = list_item("  - [ ] buy milk").expect("an item");
        assert_eq!((item.indent.as_str(), item.marker.as_str(), item.gap.as_str()), ("  ", "-", " [ ] "));
        assert_eq!(list_item("12. twelve").map(|item| item.marker), Some("12.".to_string()));
        assert_eq!(list_item("-not an item"), None);
        assert_eq!(list_item("2024 was a year"), None);
        assert_eq!(heading_level("## Two"), Some(2));
        assert_eq!(heading_level("#hashtag"), None);
    }
}
