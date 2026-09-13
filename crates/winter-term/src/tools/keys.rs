//! Keys: every built-in command beside the chord bound to it, read from the
//! keymap in force rather than from a hand-maintained list.

use crate::model::input::{Key, KeyCode, WindowKeymap};
use crate::model::page::{
    scroll_to_cursor, Page, PageContent, PageOutcome, PageRow, PageSpan, PageStyle,
};
use crate::model::palette::builtin_commands;

// ========================================================================
// Constants
// ========================================================================

/// Column the chord column starts at, wide enough for the longest label.
const CHORD_COL: usize = 44;

/// Rows of title and spacer above the first command row.
const HEADER_ROWS: usize = 2;

/// Hint shown beside the title.
const HINT: &str = "j/k move, q close";

/// Left margin every row starts at.
const LEFT_PAD: usize = 2;

/// Shown in the chord column for a command reachable only from the palette.
const NO_CHORD: &str = "-";

/// The page's title.
const TITLE: &str = "Keys";

// ========================================================================
// Data Structures
// ========================================================================

/// A page listing what Winter can do and how to ask for it.
#[derive(Clone, Debug, Default)]
pub struct KeysPage {
    commands: Vec<CommandRow>,
    /// First listed command visible in the pane.
    scroll: usize,
    selected: usize,
}

/// One listed command: what it does, and the chord that runs it.
#[derive(Clone, Debug, Eq, PartialEq)]
struct CommandRow {
    chord: String,
    label: String,
}

// ========================================================================
// KeysPage
// ========================================================================

impl KeysPage {
    /// Build the page against the bindings currently in force.
    pub fn new(keymap: &WindowKeymap) -> Self {
        let commands = builtin_commands(keymap)
            .into_iter()
            .map(|entry| CommandRow {
                chord: entry.shortcut,
                label: entry.label,
            })
            .collect();
        Self {
            commands,
            scroll: 0,
            selected: 0,
        }
    }

    fn move_down(&mut self) {
        let last = self.commands.len().saturating_sub(1);
        self.selected = (self.selected + 1).min(last);
    }

    fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn title_row(&self) -> PageRow {
        vec![
            PageSpan::new(PageStyle::Header, format!("{}{TITLE}  ", pad(LEFT_PAD))),
            PageSpan::new(PageStyle::Dim, HINT),
        ]
    }

    fn command_row(&self, command: &CommandRow) -> PageRow {
        let label_width = CHORD_COL.saturating_sub(LEFT_PAD);
        let label = format!("{}{:label_width$}", pad(LEFT_PAD), command.label);
        let chord = if command.chord.is_empty() {
            NO_CHORD
        } else {
            &command.chord
        };
        vec![
            PageSpan::plain(label),
            PageSpan::new(PageStyle::Accent, chord),
        ]
    }
}

impl Page for KeysPage {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn content(&mut self, rows: usize) -> PageContent {
        let visible = rows.saturating_sub(HEADER_ROWS);
        self.scroll = scroll_to_cursor(self.scroll, self.selected, self.commands.len(), visible);
        let mut page_rows = vec![self.title_row(), PageRow::new()];
        page_rows.extend(
            self.commands
                .iter()
                .skip(self.scroll)
                .take(visible)
                .map(|cmd| self.command_row(cmd)),
        );
        PageContent::new(page_rows).with_cursor_line(HEADER_ROWS + self.selected - self.scroll)
    }

    fn on_key(&mut self, key: &Key) -> PageOutcome {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_down();
                PageOutcome::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_up();
                PageOutcome::Consumed
            }
            KeyCode::Char('q') | KeyCode::Escape => PageOutcome::Close,
            // Every other key belongs to the host, so a new `KeyCode` variant
            // never forces a page to grow an arm for it.
            _ => PageOutcome::Ignored,
        }
    }
}

// ========================================================================
// Helpers
// ========================================================================

fn pad(width: usize) -> String {
    " ".repeat(width)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn page_with_rows(count: usize) -> KeysPage {
        let commands = (0..count)
            .map(|i| CommandRow {
                chord: String::new(),
                label: format!("command {i}"),
            })
            .collect();
        KeysPage {
            commands,
            scroll: 0,
            selected: 0,
        }
    }

    fn press(code: KeyCode) -> Key {
        Key {
            alt: false,
            code,
            ctrl: false,
            shift: false,
        }
    }

    #[test]
    fn test_cursor_stops_on_the_last_command() {
        let mut page = page_with_rows(2);
        for _ in 0..5 {
            page.on_key(&press(KeyCode::Char('j')));
        }
        assert_eq!(page.selected, 1);
    }

    #[test]
    fn test_cursor_stops_on_the_first_command() {
        let mut page = page_with_rows(2);
        page.on_key(&press(KeyCode::Char('j')));
        for _ in 0..5 {
            page.on_key(&press(KeyCode::Char('k')));
        }
        assert_eq!(page.selected, 0);
    }

    #[test]
    fn test_empty_page_keeps_the_cursor_at_zero() {
        // `len() - 1` on an empty list would underflow to usize::MAX and put
        // the cursor line far below the page.
        let mut page = page_with_rows(0);
        page.on_key(&press(KeyCode::Char('j')));
        assert_eq!(page.selected, 0);
    }

    #[test]
    fn test_cursor_line_sits_on_the_selected_command() {
        let mut page = page_with_rows(3);
        page.on_key(&press(KeyCode::Char('j')));
        assert_eq!(page.content(40).cursor_line, Some(HEADER_ROWS + 1));
    }
}
