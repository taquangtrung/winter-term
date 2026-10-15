//! Keys: every built-in command beside the chord bound to it, presented in a
//! read-only text editor pane supporting the full set of modal motions.

use crate::model::input::{Key, KeyCode, WindowKeymap};
use crate::model::page::{
    row_height, wheel_scroll, window_anchor, wrap_window, wrapped_lines, Page, PageCaret,
    PageContent, PageHint, PageNotice, PageOutcome, PagePoint, PageRow, PageSpan, PageStyle,
    PromptMode, PromptReply, PromptRequest,
};
use crate::model::palette::builtin_commands;
use crate::model::textedit::buffer::TextBuffer;
use crate::model::textedit::modal::{EditMode, EditOutcome, EditView, Marks, ModalEditor};

// ========================================================================
// Constants
// ========================================================================

/// The question the page asks for forward search.
const ASK_SEARCH: &str = "search";

/// The question the page asks for backward search.
const ASK_SEARCH_BACK: &str = "search-back";

/// Column the chord column starts at, wide enough for the longest label.
const CHORD_COL: usize = 44;

/// Rows of title and spacer above the first command row.
const HEADER_ROWS: usize = 2;

/// Hint shown beside the title.
const HINT: &str = "Enter run, / search, y yank";

/// Left margin every row starts at.
const LEFT_PAD: usize = 2;

/// Shown in the chord column for a command reachable only from the palette.
const NO_CHORD: &str = "-";

/// The page's title.
const TITLE: &str = "Keys";

// ========================================================================
// Data Structures
// ========================================================================

/// A read-only text editor page listing what Winter can do and how to ask for it.
#[derive(Clone, Debug)]
pub struct KeysPage {
    buffer: TextBuffer,
    commands: Vec<CommandRow>,
    hscroll: usize,
    marks: Marks,
    modal: ModalEditor,
    painted: Vec<Painted>,
    scroll: usize,
    viewport: usize,
    wheel: bool,
}

/// One listed command: what it does, the chord that runs it, and the name the
/// host knows it by.
#[derive(Clone, Debug, Eq, PartialEq)]
struct CommandRow {
    action: String,
    chord: String,
    label: String,
}

/// What one screen row of a painted frame holds for click targeting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Painted {
    line: usize,
    offset: usize,
}

// ========================================================================
// KeysPage
// ========================================================================

impl Default for KeysPage {
    fn default() -> Self {
        Self {
            buffer: TextBuffer::new(Vec::new()),
            commands: Vec::new(),
            hscroll: 0,
            marks: Marks::default(),
            modal: ModalEditor::read_only(),
            painted: Vec::new(),
            scroll: 0,
            viewport: 0,
            wheel: false,
        }
    }
}

impl KeysPage {
    /// Build the page against the bindings currently in force.
    pub fn new(keymap: &WindowKeymap) -> Self {
        let commands: Vec<CommandRow> = builtin_commands(keymap)
            .into_iter()
            .map(|entry| CommandRow {
                action: entry.action,
                chord: entry.shortcut,
                label: entry.label,
            })
            .collect();
        let lines: Vec<String> = commands.iter().map(command_line).collect();
        let buffer = TextBuffer::new(lines);
        Self {
            buffer,
            commands,
            hscroll: 0,
            marks: Marks::default(),
            modal: ModalEditor::read_only(),
            painted: Vec::new(),
            scroll: 0,
            viewport: 0,
            wheel: false,
        }
    }

    /// The command index currently selected by the cursor.
    pub fn selected(&self) -> usize {
        self.buffer.row()
    }

    fn title_row(&self) -> PageRow {
        let mut spans = vec![
            PageSpan::new(PageStyle::Header, format!("{}{TITLE}  ", pad(LEFT_PAD))),
            PageSpan::new(PageStyle::Dim, HINT),
        ];
        if !self.commands.is_empty() {
            spans.push(PageSpan::new(
                PageStyle::Dim,
                format!(
                    "  {}:{}  {} commands",
                    self.buffer.row() + 1,
                    self.buffer.col() + 1,
                    self.commands.len()
                ),
            ));
        }
        if let Some(name) = mode_name(self.modal.mode()) {
            spans.push(PageSpan::new(PageStyle::Accent, format!("  {name}")));
        }
        if let Some(register) = self.modal.register_prefix() {
            spans.push(PageSpan::new(PageStyle::Dim, format!("  \"{register}")));
        }
        spans
    }

    fn command_row(&self, index: usize, line: &str) -> PageRow {
        let marked = self.selected_columns(index);
        let from = line.chars().count().min(self.hscroll);
        let marked = marked.map(|(s, e)| (s.saturating_sub(from), e.saturating_sub(from)));
        let mut spans: Vec<PageSpan> = Vec::new();
        for (col, c) in line.chars().skip(from).enumerate() {
            let actual_col = col + from;
            let selected = marked.is_some_and(|(start, end)| col >= start && col < end);
            let style = if selected {
                PageStyle::Marked
            } else if actual_col >= CHORD_COL {
                PageStyle::Accent
            } else {
                PageStyle::Normal
            };
            match spans.last_mut() {
                Some(last) if last.style == style => last.text.push(c),
                _ => spans.push(PageSpan::new(style, c.to_string())),
            }
        }
        if spans.is_empty() {
            spans.push(PageSpan::plain(String::new()));
        }
        spans
    }

    fn selected_columns(&self, index: usize) -> Option<(usize, usize)> {
        let selection = self.modal.selection(&self.buffer)?;
        if index < selection.start.0 || index > selection.end.0 {
            return None;
        }
        let width = self
            .buffer
            .lines()
            .get(index)
            .map_or(0, |l| l.chars().count());
        if selection.linewise {
            return Some((0, width.max(1)));
        }
        let start = match index == selection.start.0 {
            true => selection.start.1,
            false => 0,
        };
        let end = match index == selection.end.0 {
            true => selection.end.1 + 1,
            false => width.max(1),
        };
        Some((start, end))
    }

    fn follow_cursor(&mut self, cols: usize, wrap: bool) {
        if wrap || cols == 0 {
            self.hscroll = 0;
            return;
        }
        let col = self.buffer.col();
        if col < self.hscroll {
            self.hscroll = col;
        } else if col >= self.hscroll + cols {
            self.hscroll = col + 1 - cols;
        }
    }

    fn remember_layout(&mut self, lines: &[usize], cols: usize, wrap: bool) {
        let mut painted = vec![
            Painted {
                line: lines.first().copied().unwrap_or(0),
                offset: 0,
            };
            HEADER_ROWS
        ];
        for &line in lines {
            let text = self.buffer.lines().get(line).map_or("", String::as_str);
            let chars: Vec<char> = text.chars().collect();
            for (start, _) in wrapped_lines(&chars, cols, wrap, 0) {
                painted.push(Painted {
                    line,
                    offset: match wrap {
                        true => start,
                        false => self.hscroll,
                    },
                });
            }
        }
        self.painted = painted;
    }

    fn point_at(&mut self, at: PagePoint) -> PageOutcome {
        let Some(spot) = self.painted.get(at.row).copied() else {
            return PageOutcome::Ignored;
        };
        let col = spot.offset + at.col;
        if at.drag {
            if !self.modal.is_selecting() {
                self.modal.start_visual(&self.buffer, false);
            }
        } else {
            self.modal.clear_selection();
        }
        self.buffer.move_to(spot.line, col);
        PageOutcome::Consumed
    }
}

// ========================================================================
// Page
// ========================================================================

impl Page for KeysPage {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn caret(&self) -> Option<PageCaret> {
        if self.commands.is_empty() {
            return None;
        }
        Some(PageCaret {
            col: self.buffer.col().saturating_sub(self.hscroll),
            insert: false,
        })
    }

    fn hint(&self) -> Option<PageHint> {
        Some(PageHint {
            items: self.modal.hint()?,
            title: "keys".to_string(),
        })
    }

    fn on_mouse(&mut self, at: PagePoint) -> PageOutcome {
        self.wheel = false;
        self.point_at(at)
    }

    fn on_scroll(&mut self, lines: isize) -> PageOutcome {
        let total = self.buffer.lines().len();
        self.scroll = wheel_scroll(self.scroll, lines, total, self.viewport);
        self.wheel = true;
        PageOutcome::Consumed
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> PageContent {
        self.viewport = rows.saturating_sub(HEADER_ROWS);
        self.follow_cursor(cols, wrap);
        let total = self.buffer.lines().len();
        let anchor = window_anchor(self.wheel, self.scroll, self.buffer.row(), self.viewport);
        let window = wrap_window(self.scroll, anchor, total, self.viewport, |index| {
            row_height(
                self.buffer.lines().get(index).map_or("", String::as_str),
                cols,
                wrap,
                0,
            )
        });
        self.scroll = window.start;
        let mut page_rows = vec![self.title_row(), PageRow::new()];
        let lines: Vec<usize> = (window.start..(window.start + window.count).min(total)).collect();
        for &index in &lines {
            let line = self.buffer.lines().get(index).map_or("", String::as_str);
            page_rows.push(self.command_row(index, line));
        }
        self.remember_layout(&lines, cols, wrap);
        let content = PageContent::new(page_rows);
        if self.commands.is_empty() {
            return content;
        }
        match anchor == self.buffer.row() {
            true => content.with_cursor_line(HEADER_ROWS + window.cursor),
            false => content,
        }
    }

    fn take_notice(&mut self) -> Option<PageNotice> {
        self.modal.take_notice()
    }

    fn on_key(&mut self, key: &Key) -> PageOutcome {
        self.wheel = false;

        // Enter on a command line in Normal mode runs the action.
        if self.modal.mode() == EditMode::Normal
            && key.code == KeyCode::Enter
            && !key.ctrl
            && !key.alt
        {
            let row = self.buffer.row();
            return match self.commands.get(row) {
                Some(command) => PageOutcome::RunAction(command.action.clone()),
                None => PageOutcome::Consumed,
            };
        }

        // 'q' in Normal mode when idle closes the page.
        if self.modal.is_idle() && key.code == KeyCode::Char('q') && !key.ctrl && !key.alt {
            return PageOutcome::Close;
        }

        let reg_before = self.buffer.register().clone();
        let view = EditView {
            buffer: &mut self.buffer,
            marks: &mut self.marks,
            others: &[],
            scroll: &mut self.scroll,
            viewport: self.viewport,
        };
        let outcome = self.modal.on_key(view, key);
        let reg_after = self.buffer.register();
        if reg_after != &reg_before && !reg_after.text.is_empty() {
            return PageOutcome::Yank(reg_after.text.clone());
        }
        match outcome {
            EditOutcome::Consumed => PageOutcome::Consumed,
            EditOutcome::Ignored => PageOutcome::Ignored,
            EditOutcome::Search { forward } => PageOutcome::Prompt(PromptRequest {
                initial: String::new(),
                label: if forward {
                    "/".to_string()
                } else {
                    "?".to_string()
                },
                mode: PromptMode::Text,
                tag: if forward { ASK_SEARCH } else { ASK_SEARCH_BACK },
            }),
            EditOutcome::Yank(text) => PageOutcome::Yank(text),
            EditOutcome::Quit { .. } => PageOutcome::Close,
            _ => PageOutcome::Consumed,
        }
    }

    fn on_prompt(&mut self, reply: PromptReply) -> PageOutcome {
        self.wheel = false;
        if let Some(pattern) = reply.answer {
            let forward = reply.tag == ASK_SEARCH;
            let view = EditView {
                buffer: &mut self.buffer,
                marks: &mut self.marks,
                others: &[],
                scroll: &mut self.scroll,
                viewport: self.viewport,
            };
            self.modal.start_search(view, pattern, forward);
        }
        PageOutcome::Consumed
    }

    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> PageOutcome {
        if (tag == ASK_SEARCH || tag == ASK_SEARCH_BACK) && !query.is_empty() {
            let forward = tag == ASK_SEARCH;
            let view = EditView {
                buffer: &mut self.buffer,
                marks: &mut self.marks,
                others: &[],
                scroll: &mut self.scroll,
                viewport: self.viewport,
            };
            self.modal.start_search(view, query.to_string(), forward);
        }
        PageOutcome::Consumed
    }
}

// ========================================================================
// Helpers
// ========================================================================

fn command_line(command: &CommandRow) -> String {
    let label_width = CHORD_COL.saturating_sub(LEFT_PAD);
    let chord = if command.chord.is_empty() {
        NO_CHORD
    } else {
        &command.chord
    };
    format!("{}{:label_width$}{}", pad(LEFT_PAD), command.label, chord)
}

fn pad(width: usize) -> String {
    " ".repeat(width)
}

fn mode_name(mode: EditMode) -> Option<&'static str> {
    match mode {
        EditMode::Insert => Some("INSERT"),
        EditMode::Normal => None,
        EditMode::Replace => Some("REPLACE"),
        EditMode::Visual => Some("VISUAL"),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn page_with_rows(count: usize) -> KeysPage {
        let commands: Vec<CommandRow> = (0..count)
            .map(|i| CommandRow {
                action: format!("action_{i}"),
                chord: String::new(),
                label: format!("command {i}"),
            })
            .collect();
        let lines: Vec<String> = commands.iter().map(command_line).collect();
        let buffer = TextBuffer::new(lines);
        KeysPage {
            buffer,
            commands,
            hscroll: 0,
            marks: Marks::default(),
            modal: ModalEditor::read_only(),
            painted: Vec::new(),
            scroll: 0,
            viewport: 0,
            wheel: false,
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
    fn test_the_emacs_buffer_ends_reach_the_first_and_last_command() {
        let alt = |c: char| Key {
            alt: true,
            code: KeyCode::Char(c),
            ctrl: false,
            shift: false,
        };
        let mut page = page_with_rows(5);
        page.on_key(&alt('>'));
        assert_eq!(page.selected(), 4);
        page.on_key(&alt('<'));
        assert_eq!(page.selected(), 0);
    }

    #[test]
    fn test_cursor_stops_on_the_last_command() {
        let mut page = page_with_rows(2);
        for _ in 0..5 {
            page.on_key(&press(KeyCode::Char('j')));
        }
        assert_eq!(page.selected(), 1);
    }

    #[test]
    fn test_cursor_stops_on_the_first_command() {
        let mut page = page_with_rows(2);
        page.on_key(&press(KeyCode::Char('j')));
        for _ in 0..5 {
            page.on_key(&press(KeyCode::Char('k')));
        }
        assert_eq!(page.selected(), 0);
    }

    #[test]
    fn test_empty_page_keeps_the_cursor_at_zero() {
        let mut page = page_with_rows(0);
        page.on_key(&press(KeyCode::Char('j')));
        assert_eq!(page.selected(), 0);
    }

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
        assert_eq!(page.selected(), 3);
    }

    #[test]
    fn test_the_repeat_keys_step_through_the_matches_in_both_directions() {
        let mut page = page_with_rows(5);
        search(&mut page, "command");
        assert_eq!(page.selected(), 0);

        page.on_key(&press(KeyCode::Char('n')));
        assert_eq!(page.selected(), 1);

        page.on_key(&press(KeyCode::Char('N')));
        assert_eq!(page.selected(), 0);
    }

    #[test]
    fn test_a_search_finds_a_command_by_the_chord_beside_it() {
        let commands = vec![
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
        ];
        let lines: Vec<String> = commands.iter().map(command_line).collect();
        let buffer = TextBuffer::new(lines);
        let mut page = KeysPage {
            buffer,
            commands,
            hscroll: 0,
            marks: Marks::default(),
            modal: ModalEditor::read_only(),
            painted: Vec::new(),
            scroll: 0,
            viewport: 0,
            wheel: false,
        };
        search(&mut page, "c+s+g");
        assert_eq!(page.selected(), 1);
    }

    #[test]
    fn test_enter_hands_back_the_command_under_the_cursor() {
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
        assert!(page.take_notice().is_some());

        search(&mut page, "absent");
        assert!(page.take_notice().is_some());
        assert_eq!(page.selected(), 0);
    }

    #[test]
    fn test_an_empty_list_has_no_cursor_line() {
        let mut page = page_with_rows(0);
        assert_eq!(page.content(20, 80, false).cursor_line, None);
    }

    #[test]
    fn test_cursor_line_sits_on_the_selected_command() {
        let mut page = page_with_rows(3);
        page.on_key(&press(KeyCode::Char('j')));
        assert_eq!(
            page.content(40, 80, false).cursor_line,
            Some(HEADER_ROWS + 1)
        );
    }

    #[test]
    fn test_vim_motions_work_in_keybinding_view() {
        let mut page = page_with_rows(10);
        // G goes to the last line
        page.on_key(&press(KeyCode::Char('G')));
        assert_eq!(page.selected(), 9);

        // gg goes to the first line (landing at first non-blank column 2)
        page.on_key(&press(KeyCode::Char('g')));
        page.on_key(&press(KeyCode::Char('g')));
        assert_eq!(page.selected(), 0);
        assert_eq!(page.buffer.col(), 2);

        // 0 moves to column 0
        page.on_key(&press(KeyCode::Char('0')));
        assert_eq!(page.buffer.col(), 0);

        // w moves forward words on the line
        page.on_key(&press(KeyCode::Char('w')));
        assert!(page.buffer.col() > 0);
    }

    #[test]
    fn test_read_only_protection_in_keybinding_view() {
        let mut page = page_with_rows(3);
        let orig_line = page.buffer.lines()[0].clone();

        // Typing 'i', 'x', 'd', 'p' should not change buffer
        page.on_key(&press(KeyCode::Char('i')));
        assert_eq!(
            page.take_notice().map(|notice| notice.text).as_deref(),
            Some("buffer is read-only")
        );
        page.on_key(&press(KeyCode::Char('x')));
        assert_eq!(
            page.take_notice().map(|notice| notice.text).as_deref(),
            Some("buffer is read-only")
        );
        page.on_key(&press(KeyCode::Char('d')));
        assert_eq!(
            page.take_notice().map(|notice| notice.text).as_deref(),
            Some("buffer is read-only")
        );

        assert_eq!(page.buffer.lines()[0], orig_line);
    }

    #[test]
    fn test_visual_mode_and_yank_in_keybinding_view() {
        let mut page = page_with_rows(3);
        // Start visual mode
        page.on_key(&press(KeyCode::Char('v')));
        assert_eq!(page.modal.mode(), EditMode::Visual);

        // Yank selection
        let outcome = page.on_key(&press(KeyCode::Char('y')));
        assert_eq!(page.modal.mode(), EditMode::Normal);
        assert!(matches!(outcome, PageOutcome::Yank(_)));
    }

    #[test]
    fn test_q_closes_keybinding_view() {
        let mut page = page_with_rows(3);
        assert_eq!(page.on_key(&press(KeyCode::Char('q'))), PageOutcome::Close);
    }
}
