//! Keys: every built-in command beside the chord bound to it, presented in a
//! read-only text editor pane supporting the full set of modal motions.

use crate::model::input::{Key, KeyCode, WindowKeymap};
use crate::model::paint::{
    row_height, wheel_scroll, window_anchor, wrap_window, wrapped_lines, Caret, Frame, Point, Row,
    Span, Style,
};
use crate::model::palette::builtin_commands;
use crate::model::textedit::buffer::TextBuffer;
use crate::model::textedit::modal::{EditMode, EditOutcome, EditView, Marks, ModalEditor};
use crate::model::tool::{
    PromptMode, PromptReply, PromptRequest, Tool, ToolHint, ToolNotice, ToolOutcome, ToolStatus,
};

// ========================================================================
// Constants
// ========================================================================

/// The question the tool asks for forward search.
const ASK_SEARCH: &str = "search";

/// The question the tool asks for backward search.
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

/// The tool's title.
const TITLE: &str = "Keys";

// ========================================================================
// Data Structures
// ========================================================================

/// A read-only text editor tool listing what Winter can do and how to ask for it.
#[derive(Clone, Debug)]
pub struct KeysTool {
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
// KeysTool
// ========================================================================

impl Default for KeysTool {
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

impl KeysTool {
    /// Build the tool against the bindings currently in force.
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

    /// The title and the key hints. Where the cursor is and the mode are the
    /// status bar's: see [`Tool::status`].
    fn title_row(&self) -> Row {
        vec![
            Span::new(Style::Header, format!("{}{TITLE}  ", pad(LEFT_PAD))),
            Span::new(Style::Dim, HINT),
        ]
    }

    fn command_row(&self, index: usize, line: &str) -> Row {
        let marked = self.selected_columns(index);
        let from = line.chars().count().min(self.hscroll);
        let marked = marked.map(|(s, e)| (s.saturating_sub(from), e.saturating_sub(from)));
        let mut spans: Vec<Span> = Vec::new();
        for (col, c) in line.chars().skip(from).enumerate() {
            let actual_col = col + from;
            let selected = marked.is_some_and(|(start, end)| col >= start && col < end);
            let style = if selected {
                Style::Marked
            } else if actual_col >= CHORD_COL {
                Style::Accent
            } else {
                Style::Normal
            };
            match spans.last_mut() {
                Some(last) if last.style == style => last.text.push(c),
                _ => spans.push(Span::new(style, c.to_string())),
            }
        }
        if spans.is_empty() {
            spans.push(Span::plain(String::new()));
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

    fn point_at(&mut self, at: Point) -> ToolOutcome {
        let Some(spot) = self.painted.get(at.row).copied() else {
            return ToolOutcome::Ignored;
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
        ToolOutcome::Consumed
    }
}

// ========================================================================
// Tool
// ========================================================================

impl Tool for KeysTool {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn caret(&self) -> Option<Caret> {
        if self.commands.is_empty() {
            return None;
        }
        Some(Caret {
            col: self.buffer.col().saturating_sub(self.hscroll),
            insert: false,
        })
    }

    fn hint(&self) -> Option<ToolHint> {
        Some(ToolHint {
            items: self.modal.hint()?,
            title: "keys".to_string(),
        })
    }

    fn on_mouse(&mut self, at: Point) -> ToolOutcome {
        self.wheel = false;
        self.point_at(at)
    }

    fn on_scroll(&mut self, lines: isize) -> ToolOutcome {
        let total = self.buffer.lines().len();
        self.scroll = wheel_scroll(self.scroll, lines, total, self.viewport);
        self.wheel = true;
        ToolOutcome::Consumed
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> Frame {
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
        let mut frame_rows = vec![self.title_row(), Row::new()];
        let lines: Vec<usize> = (window.start..(window.start + window.count).min(total)).collect();
        for &index in &lines {
            let line = self.buffer.lines().get(index).map_or("", String::as_str);
            frame_rows.push(self.command_row(index, line));
        }
        self.remember_layout(&lines, cols, wrap);
        let content = Frame::new(frame_rows);
        if self.commands.is_empty() {
            return content;
        }
        match anchor == self.buffer.row() {
            true => content.with_cursor_line(HEADER_ROWS + window.cursor),
            false => content,
        }
    }

    fn take_notice(&mut self) -> Option<ToolNotice> {
        self.modal.take_notice()
    }

    fn status(&self) -> ToolStatus {
        let total = format!("{} commands", self.commands.len());
        self.modal.tool_status(&self.buffer, total)
    }

    fn on_key(&mut self, key: &Key) -> ToolOutcome {
        self.wheel = false;

        // Enter on a command line in Normal mode runs the action.
        if self.modal.mode() == EditMode::Normal
            && key.glyph() == KeyCode::Enter
            && !key.ctrl
            && !key.alt
        {
            let row = self.buffer.row();
            return match self.commands.get(row) {
                Some(command) => ToolOutcome::RunAction(command.action.clone()),
                None => ToolOutcome::Consumed,
            };
        }

        // 'q' in Normal mode when idle closes the tool.
        if self.modal.is_idle() && key.glyph() == KeyCode::Char('q') && !key.ctrl && !key.alt {
            return ToolOutcome::Close;
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
            return ToolOutcome::Yank(reg_after.text.clone());
        }
        match outcome {
            EditOutcome::Consumed => ToolOutcome::Consumed,
            EditOutcome::Ignored => ToolOutcome::Ignored,
            EditOutcome::Search { forward } => ToolOutcome::Prompt(PromptRequest {
                initial: String::new(),
                label: if forward {
                    "/".to_string()
                } else {
                    "?".to_string()
                },
                mode: PromptMode::Text,
                tag: if forward { ASK_SEARCH } else { ASK_SEARCH_BACK },
            }),
            EditOutcome::Yank(text) => ToolOutcome::Yank(text),
            EditOutcome::Quit { .. } => ToolOutcome::Close,
            _ => ToolOutcome::Consumed,
        }
    }

    fn on_prompt(&mut self, reply: PromptReply) -> ToolOutcome {
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
        ToolOutcome::Consumed
    }

    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> ToolOutcome {
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
        ToolOutcome::Consumed
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

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_with_rows(count: usize) -> KeysTool {
        let commands: Vec<CommandRow> = (0..count)
            .map(|i| CommandRow {
                action: format!("action_{i}"),
                chord: String::new(),
                label: format!("command {i}"),
            })
            .collect();
        let lines: Vec<String> = commands.iter().map(command_line).collect();
        let buffer = TextBuffer::new(lines);
        KeysTool {
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
            sup: false,
        }
    }

    #[test]
    fn test_the_emacs_buffer_ends_reach_the_first_and_last_command() {
        let alt = |c: char| Key {
            alt: true,
            code: KeyCode::Char(c),
            ctrl: false,
            shift: false,
            sup: false,
        };
        let mut tool = tool_with_rows(5);
        tool.on_key(&Key {
            shift: true,
            ..alt('>')
        });
        assert_eq!(tool.selected(), 4);
        tool.on_key(&Key {
            shift: true,
            ..alt('<')
        });
        assert_eq!(tool.selected(), 0);
    }

    #[test]
    fn test_cursor_stops_on_the_last_command() {
        let mut tool = tool_with_rows(2);
        for _ in 0..5 {
            tool.on_key(&press(KeyCode::Char('j')));
        }
        assert_eq!(tool.selected(), 1);
    }

    #[test]
    fn test_cursor_stops_on_the_first_command() {
        let mut tool = tool_with_rows(2);
        tool.on_key(&press(KeyCode::Char('j')));
        for _ in 0..5 {
            tool.on_key(&press(KeyCode::Char('k')));
        }
        assert_eq!(tool.selected(), 0);
    }

    #[test]
    fn test_empty_tool_keeps_the_cursor_at_zero() {
        let mut tool = tool_with_rows(0);
        tool.on_key(&press(KeyCode::Char('j')));
        assert_eq!(tool.selected(), 0);
    }

    fn search(tool: &mut KeysTool, text: &str) {
        let outcome = tool.on_key(&press(KeyCode::Char('/')));
        let ToolOutcome::Prompt(request) = outcome else {
            panic!("expected a prompt, got {outcome:?}");
        };
        tool.on_prompt(PromptReply {
            answer: Some(text.to_string()),
            tag: request.tag,
        });
    }

    #[test]
    fn test_a_search_moves_the_cursor_to_the_matching_command() {
        let mut tool = tool_with_rows(5);
        search(&mut tool, "command 3");
        assert_eq!(tool.selected(), 3);
    }

    #[test]
    fn test_the_repeat_keys_step_through_the_matches_in_both_directions() {
        let mut tool = tool_with_rows(5);
        search(&mut tool, "command");
        assert_eq!(tool.selected(), 0);

        tool.on_key(&press(KeyCode::Char('n')));
        assert_eq!(tool.selected(), 1);

        tool.on_key(&press(KeyCode::Char('N')));
        assert_eq!(tool.selected(), 0);
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
        let mut tool = KeysTool {
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
        search(&mut tool, "c+s+g");
        assert_eq!(tool.selected(), 1);
    }

    #[test]
    fn test_enter_hands_back_the_command_under_the_cursor() {
        let mut tool = tool_with_rows(3);
        tool.on_key(&press(KeyCode::Char('j')));
        assert_eq!(
            tool.on_key(&press(KeyCode::Enter)),
            ToolOutcome::RunAction("action_1".to_string())
        );
    }

    #[test]
    fn test_enter_with_nothing_listed_runs_nothing() {
        let mut tool = tool_with_rows(0);
        assert_eq!(tool.on_key(&press(KeyCode::Enter)), ToolOutcome::Consumed);
    }

    #[test]
    fn test_a_search_with_nothing_to_find_reports_it() {
        let mut tool = tool_with_rows(2);
        tool.on_key(&press(KeyCode::Char('n')));
        assert!(tool.take_notice().is_some());

        search(&mut tool, "absent");
        assert!(tool.take_notice().is_some());
        assert_eq!(tool.selected(), 0);
    }

    #[test]
    fn test_an_empty_list_has_no_cursor_line() {
        let mut tool = tool_with_rows(0);
        assert_eq!(tool.content(20, 80, false).cursor_line, None);
    }

    #[test]
    fn test_cursor_line_sits_on_the_selected_command() {
        let mut tool = tool_with_rows(3);
        tool.on_key(&press(KeyCode::Char('j')));
        assert_eq!(
            tool.content(40, 80, false).cursor_line,
            Some(HEADER_ROWS + 1)
        );
    }

    #[test]
    fn test_vim_motions_work_in_keybinding_view() {
        let mut tool = tool_with_rows(10);
        // G goes to the last line
        tool.on_key(&press(KeyCode::Char('G')));
        assert_eq!(tool.selected(), 9);

        // gg goes to the first line (landing at first non-blank column 2)
        tool.on_key(&press(KeyCode::Char('g')));
        tool.on_key(&press(KeyCode::Char('g')));
        assert_eq!(tool.selected(), 0);
        assert_eq!(tool.buffer.col(), 2);

        // 0 moves to column 0
        tool.on_key(&press(KeyCode::Char('0')));
        assert_eq!(tool.buffer.col(), 0);

        // w moves forward words on the line
        tool.on_key(&press(KeyCode::Char('w')));
        assert!(tool.buffer.col() > 0);
    }

    #[test]
    fn test_read_only_protection_in_keybinding_view() {
        let mut tool = tool_with_rows(3);
        let orig_line = tool.buffer.lines()[0].clone();

        // Typing 'i', 'x', 'd', 'p' should not change buffer
        tool.on_key(&press(KeyCode::Char('i')));
        assert_eq!(
            tool.take_notice().map(|notice| notice.text).as_deref(),
            Some("buffer is read-only")
        );
        tool.on_key(&press(KeyCode::Char('x')));
        assert_eq!(
            tool.take_notice().map(|notice| notice.text).as_deref(),
            Some("buffer is read-only")
        );
        tool.on_key(&press(KeyCode::Char('d')));
        assert_eq!(
            tool.take_notice().map(|notice| notice.text).as_deref(),
            Some("buffer is read-only")
        );

        assert_eq!(tool.buffer.lines()[0], orig_line);
    }

    #[test]
    fn test_visual_mode_and_yank_in_keybinding_view() {
        let mut tool = tool_with_rows(3);
        // Start visual mode
        tool.on_key(&press(KeyCode::Char('v')));
        assert_eq!(tool.modal.mode(), EditMode::Visual);

        // Yank selection
        let outcome = tool.on_key(&press(KeyCode::Char('y')));
        assert_eq!(tool.modal.mode(), EditMode::Normal);
        assert!(matches!(outcome, ToolOutcome::Yank(_)));
    }

    #[test]
    fn test_q_closes_keybinding_view() {
        let mut tool = tool_with_rows(3);
        assert_eq!(tool.on_key(&press(KeyCode::Char('q'))), ToolOutcome::Close);
    }
}
