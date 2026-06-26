//! Keys: every built-in command beside the chord bound to it, read from the
//! keymap in force rather than from a hand-maintained list.

use crate::model::input::{Key, KeyCode, WindowKeymap};
use crate::model::page::{
    find_match, row_text, scroll_to_cursor, Page, PageContent, PageOutcome, PageRow, PageSpan,
    PageStyle, PromptMode, PromptReply, PromptRequest,
};
use crate::model::palette::builtin_commands;

// ========================================================================
// Constants
// ========================================================================

/// The one question the page asks.
const ASK_SEARCH: &str = "search";

/// Column the chord column starts at, wide enough for the longest label.
const CHORD_COL: usize = 44;

/// Rows of title and spacer above the first command row.
const HEADER_ROWS: usize = 2;

/// Hint shown beside the title.
const HINT: &str = "j/k move, / search, Enter run, q close";

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
    /// What the last search reported, shown beside the title until the next key.
    message: Option<String>,
    /// First listed command visible in the pane.
    scroll: usize,
    /// The last text searched for, repeated by the next and previous keys.
    search: String,
    selected: usize,
}

/// One listed command: what it does, the chord that runs it, and the name the
/// host knows it by.
#[derive(Clone, Debug, Eq, PartialEq)]
struct CommandRow {
    action: String,
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
                action: entry.action,
                chord: entry.shortcut,
                label: entry.label,
            })
            .collect();
        Self {
            commands,
            message: None,
            scroll: 0,
            search: String::new(),
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

    /// Remember `query` and move to the first command holding it.
    fn search_for(&mut self, query: &str) {
        self.search = query.to_string();
        self.search_step(true);
    }

    /// Move to the next command matching the last search. What is searched is
    /// what is painted, so a chord finds its command as readily as a label.
    fn search_step(&mut self, forward: bool) {
        if self.search.is_empty() {
            self.message = Some("no search".to_string());
            return;
        }
        let labels: Vec<String> = self
            .commands
            .iter()
            .map(|command| row_text(&self.command_row(command)))
            .collect();
        match find_match(&labels, &self.search, self.selected, forward) {
            Some(index) => self.selected = index,
            None => self.message = Some(format!("not found: {}", self.search)),
        }
    }

    fn title_row(&self) -> PageRow {
        let mut spans = vec![
            PageSpan::new(PageStyle::Header, format!("{}{TITLE}  ", pad(LEFT_PAD))),
            PageSpan::new(PageStyle::Dim, HINT),
        ];
        if let Some(message) = &self.message {
            spans.push(PageSpan::new(PageStyle::Dim, format!("  {message}")));
        }
        spans
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
        let content = PageContent::new(page_rows);
        if self.commands.is_empty() {
            // Nothing to band, and a cursor line under the header would light up
            // a blank row that stands for no command.
            return content;
        }
        content.with_cursor_line(HEADER_ROWS + self.selected - self.scroll)
    }

    fn on_key(&mut self, key: &Key) -> PageOutcome {
        self.message = None;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_down();
                PageOutcome::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_up();
                PageOutcome::Consumed
            }
            KeyCode::Char('/') => PageOutcome::Prompt(PromptRequest {
                initial: String::new(),
                label: "/".to_string(),
                mode: PromptMode::Text,
                tag: ASK_SEARCH,
            }),
            KeyCode::Char('n') => {
                self.search_step(true);
                PageOutcome::Consumed
            }
            KeyCode::Char('N') => {
                self.search_step(false);
                PageOutcome::Consumed
            }
            KeyCode::Enter => match self.commands.get(self.selected) {
                Some(command) => PageOutcome::RunAction(command.action.clone()),
                None => PageOutcome::Consumed,
            },
            KeyCode::Char('q') | KeyCode::Escape => PageOutcome::Close,
            // Every other key belongs to the host, so a new `KeyCode` variant
            // never forces a page to grow an arm for it.
            _ => PageOutcome::Ignored,
        }
    }

    fn on_prompt(&mut self, reply: PromptReply) -> PageOutcome {
        if let Some(answer) = reply.answer {
            self.search_for(&answer);
        }
        PageOutcome::Consumed
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
                action: format!("action_{i}"),
                chord: String::new(),
                label: format!("command {i}"),
            })
            .collect();
        KeysPage {
            commands,
            message: None,
            scroll: 0,
            search: String::new(),
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

    /// Ask for `text` through the search prompt, the way a key does.
    fn search(page: &mut KeysPage, text: &str) {
        let outcome = page.on_key(&press(KeyCode::Char('/')));
        let PageOutcome::Prompt(request) = outcome else {
            panic!("expected a prompt, got {outcome:?}");
        };
        page.on_prompt(PromptReply {
            answer: Some(text.to_string()),
            tag: request.tag,
        });
    }

    #[test]
    fn test_a_search_moves_the_cursor_to_the_matching_command() {
        let mut page = page_with_rows(5);
        search(&mut page, "command 3");
        assert_eq!(page.selected, 3);
    }

    #[test]
    fn test_the_repeat_keys_step_through_the_matches_in_both_directions() {
        let mut page = page_with_rows(5);
        search(&mut page, "command");
        assert_eq!(page.selected, 1, "the row already under the cursor is last");

        page.on_key(&press(KeyCode::Char('n')));
        assert_eq!(page.selected, 2);

        page.on_key(&press(KeyCode::Char('N')));
        assert_eq!(page.selected, 1);
    }

    #[test]
    fn test_a_search_finds_a_command_by_the_chord_beside_it() {
        // Searching the label alone would leave no way to ask what a chord you
        // just pressed by accident actually does.
        let mut page = KeysPage {
            commands: vec![
                CommandRow {
                    action: "new_tab".to_string(),
                    chord: String::new(),
                    label: "New Tab".to_string(),
                },
                CommandRow {
                    action: "git_page".to_string(),
                    chord: "C+S+g".to_string(),
                    label: "Git: Status".to_string(),
                },
            ],
            message: None,
            scroll: 0,
            search: String::new(),
            selected: 0,
        };
        search(&mut page, "c+s+g");
        assert_eq!(page.selected, 1);
    }

    #[test]
    fn test_enter_hands_back_the_command_under_the_cursor() {
        // The label is what the page shows, but the host only answers to the
        // action name, so handing back the wrong one runs nothing.
        let mut page = page_with_rows(3);
        page.on_key(&press(KeyCode::Char('j')));
        assert_eq!(
            page.on_key(&press(KeyCode::Enter)),
            PageOutcome::RunAction("action_1".to_string())
        );
    }

    #[test]
    fn test_enter_with_nothing_listed_runs_nothing() {
        let mut page = page_with_rows(0);
        assert_eq!(page.on_key(&press(KeyCode::Enter)), PageOutcome::Consumed);
    }

    #[test]
    fn test_a_search_with_nothing_to_find_reports_it() {
        let mut page = page_with_rows(2);
        page.on_key(&press(KeyCode::Char('n')));
        assert_eq!(page.message.as_deref(), Some("no search"));

        search(&mut page, "absent");
        assert_eq!(page.message.as_deref(), Some("not found: absent"));
        assert_eq!(page.selected, 0);
    }

    #[test]
    fn test_an_empty_list_has_no_cursor_line() {
        let mut page = page_with_rows(0);
        assert_eq!(page.content(20).cursor_line, None);
    }

    #[test]
    fn test_cursor_line_sits_on_the_selected_command() {
        let mut page = page_with_rows(3);
        page.on_key(&press(KeyCode::Char('j')));
        assert_eq!(page.content(40).cursor_line, Some(HEADER_ROWS + 1));
    }
}
