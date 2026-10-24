//! Keys: every built-in command beside the chord bound to it, presented in a
//! read-only text editor pane supporting the full set of modal motions.

use crate::model::input::table::{self, Binding, KeyHelp};
use crate::model::input::{Key, Keymap};
use crate::model::paint::{row_height, wrapped_lines, Caret, Frame, Point, Row, Span, Style};
use crate::model::palette::builtin_commands;
use crate::model::textedit::buffer::TextBuffer;
use crate::model::textedit::modal::{EditMode, EditOutcome, EditView, Marks, ModalEditor};
use crate::model::tool::{
    menu_groups, Notice, PromptMode, PromptReply, PromptRequest, SearchReport, Tool, ToolHint,
    ToolMenuItem, ToolOutcome, ToolStatus,
};
use crate::model::vim::nav::VimNav;
use crate::tools::list::Viewport;
use crate::tools::COMMAND_BLOCKS;

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

/// Left margin every row starts at.
const LEFT_PAD: usize = 2;

/// Shown in the chord column for a command reachable only from the palette.
const NO_CHORD: &str = "-";

/// The menu's entries typed as Vim keys: the row copied through the `+`
/// register, so it reaches other windows, and a search.
const MENU_COPY_ROW: (&str, &str) = ("\" + y y", "Copy Row");
const MENU_SEARCH: (&str, &str) = ("/", "Find");

/// The tool's title.
const TITLE: &str = "Keys";

/// The tool's own keys, ahead of the read-only editor's.
const KEYS: &[Binding<Command>] = &[
    Binding {
        action: Command::Run,
        keys: &["RET"],
        label: "Run the Command",
    },
    Binding {
        action: Command::Close,
        keys: &["q"],
        label: "Close",
    },
];

// ========================================================================
// Data Structures
// ========================================================================

/// What a key of the tool's own does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    /// Close the tool, with nothing part-typed.
    Close,
    /// Run the command on the cursor's line, in Normal mode.
    Run,
}

/// A read-only text editor tool listing what Winter can do and how to ask for it.
#[derive(Clone, Debug)]
pub struct KeysTool {
    buffer: TextBuffer,
    commands: Vec<CommandRow>,
    hscroll: usize,
    marks: Marks,
    modal: ModalEditor,
    /// A report of the tool's own, ahead of the editor's.
    notice: Option<Notice>,
    painted: Vec<Painted>,
    /// The window over the listed lines.
    view: Viewport,
}

/// One listed command: what it does, the chord that runs it, and the name the
/// host knows it by.
#[derive(Clone, Debug, Eq, PartialEq)]
struct CommandRow {
    action: String,
    chord: String,
    label: String,
    /// The tool whose tab runs the command, for a tool's own command, which
    /// cannot run from here.
    runs_in: Option<&'static str>,
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
        Self::from_rows(Vec::new())
    }
}

impl KeysTool {
    /// Build the tool against the bindings currently in force.
    pub fn new(keymap: &Keymap) -> Self {
        let builtin = builtin_commands(keymap)
            .into_iter()
            .map(|entry| CommandRow {
                action: entry.action,
                chord: entry.shortcut,
                label: entry.label,
                runs_in: None,
            });
        let tools = COMMAND_BLOCKS.iter().flat_map(|block| {
            block.commands.iter().map(|command| CommandRow {
                action: command.name.to_string(),
                chord: keymap.tool_chord_hint(block.block, command.name),
                label: format!("{}: {}", block.title, command.label),
                runs_in: Some(block.title),
            })
        });
        Self::from_rows(builtin.chain(tools).collect())
    }

    /// The tool listing `commands`, one per line.
    fn from_rows(commands: Vec<CommandRow>) -> Self {
        let lines: Vec<String> = commands.iter().map(command_line).collect();
        Self {
            buffer: TextBuffer::new(lines),
            commands,
            hscroll: 0,
            marks: Marks::default(),
            modal: ModalEditor::read_only(),
            notice: None,
            painted: Vec::new(),
            view: Viewport::new(),
        }
    }

    /// The command index currently selected by the cursor.
    pub fn selected(&self) -> usize {
        self.buffer.row()
    }

    /// The title and the key hints. Where the cursor is and the mode are the
    /// status bar's: see [`Tool::status`].
    fn title_row(&self) -> Row {
        vec![Span::new(
            Style::Header,
            format!("{}{TITLE}", pad(LEFT_PAD)),
        )]
    }

    /// Run the command on the cursor's line, or say where a tool's own one
    /// runs instead.
    fn run_row(&mut self) -> ToolOutcome {
        match self.commands.get(self.buffer.row()) {
            Some(CommandRow {
                runs_in: Some(tool),
                ..
            }) => {
                self.notice = Some(Notice::info(format!("runs in a {tool} tab")));
                ToolOutcome::Consumed
            }
            Some(command) => ToolOutcome::RunAction(command.action.clone()),
            None => ToolOutcome::Consumed,
        }
    }

    fn command_row(&self, index: usize, line: &str) -> Row {
        let marked = self.selected_columns(index);
        let from = line.chars().count().min(self.hscroll);
        let marked = marked.map(|(s, e)| (s.saturating_sub(from), e.saturating_sub(from)));
        let mut spans: Vec<Span> = Vec::new();
        for (col, c) in line.chars().skip(from).enumerate() {
            let actual_col = col + from;
            let selected = marked.is_some_and(|(start, end)| col >= start && col < end);
            let style = match actual_col >= CHORD_COL {
                true => Style::Accent,
                false => Style::Normal,
            };
            match spans.last_mut() {
                Some(last) if last.style == style && last.selected == selected => {
                    last.text.push(c)
                }
                _ => {
                    let span = Span::new(style, c.to_string());
                    spans.push(if selected { span.selected() } else { span });
                }
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
    /// The command under the pointer to run or copy, then a search, then
    /// closing.
    fn context_items(&self) -> Vec<ToolMenuItem> {
        let own = |command| {
            table::menu_entry(KEYS, command).map(|(key, label)| ToolMenuItem::new(key, label))
        };
        let vim = |(spec, label): (&str, &str)| {
            table::keys(spec).map(|keys| ToolMenuItem::typed(keys, label))
        };
        let flat = |items: Vec<Option<ToolMenuItem>>| items.into_iter().flatten().collect();
        menu_groups([
            flat(vec![own(Command::Run), vim(MENU_COPY_ROW)]),
            flat(vec![vim(MENU_SEARCH)]),
            flat(vec![own(Command::Close)]),
        ])
    }

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
        self.view.release();
        self.point_at(at)
    }

    fn on_scroll(&mut self, lines: isize) -> ToolOutcome {
        let total = self.buffer.lines().len();
        self.view.wheel(lines, total);
        ToolOutcome::Consumed
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> Frame {
        let visible = rows.saturating_sub(HEADER_ROWS);
        self.view.set_rows(visible);
        self.follow_cursor(cols, wrap);
        let total = self.buffer.lines().len();
        let lines_of = self.buffer.lines();
        let placed = self.view.place(self.buffer.row(), total, visible, |index| {
            row_height(
                lines_of.get(index).map_or("", String::as_str),
                cols,
                wrap,
                0,
            )
        });
        let window = placed.window;
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
        match placed.shows_cursor {
            true => content.with_cursor_line(HEADER_ROWS + window.cursor),
            false => content,
        }
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take().or_else(|| self.modal.take_notice())
    }

    fn take_search(&mut self) -> Option<SearchReport> {
        self.modal.take_search(&self.buffer)
    }

    fn status(&self) -> ToolStatus {
        let total = format!("{} commands", self.commands.len());
        self.modal.tool_status(&self.buffer, total)
    }

    fn on_key(&mut self, key: &Key) -> ToolOutcome {
        self.view.release();

        match table::find(KEYS, key) {
            Some(Command::Run) if self.modal.mode() == EditMode::Normal => {
                return self.run_row();
            }
            Some(Command::Close) if self.modal.is_idle() => return ToolOutcome::Close,
            _ => {}
        }

        let reg_before = self.buffer.register().clone();
        let view = EditView {
            buffer: &mut self.buffer,
            marks: &mut self.marks,
            others: &[],
            viewport: self.view.rows(),
            scroll: self.view.scroll_mut(),
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
                mode: PromptMode::Search { reverse: !forward },
                tag: if forward { ASK_SEARCH } else { ASK_SEARCH_BACK },
            }),
            EditOutcome::Yank(text) => ToolOutcome::Yank(text),
            EditOutcome::Quit { .. } => ToolOutcome::Close,
            _ => ToolOutcome::Consumed,
        }
    }

    fn keys(&self) -> Vec<KeyHelp> {
        let mut rows = table::help(KEYS);
        rows.extend(VimNav::help());
        rows
    }

    fn on_prompt(&mut self, reply: PromptReply) -> ToolOutcome {
        self.view.release();
        if let Some(pattern) = reply.answer {
            let forward = reply.tag == ASK_SEARCH;
            let view = EditView {
                buffer: &mut self.buffer,
                marks: &mut self.marks,
                others: &[],
                viewport: self.view.rows(),
                scroll: self.view.scroll_mut(),
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
                viewport: self.view.rows(),
                scroll: self.view.scroll_mut(),
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
    use crate::model::input::KeyCode;

    fn tool_with_rows(count: usize) -> KeysTool {
        let commands: Vec<CommandRow> = (0..count)
            .map(|i| CommandRow {
                action: format!("action_{i}"),
                chord: String::new(),
                label: format!("command {i}"),
                runs_in: None,
            })
            .collect();
        KeysTool::from_rows(commands)
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
    fn test_the_menu_runs_copies_finds_and_closes() {
        let tool = tool_with_rows(3);
        let labels: Vec<String> = tool
            .context_items()
            .into_iter()
            .filter(|item| !item.is_separator())
            .map(|item| item.label)
            .collect();
        assert_eq!(labels, ["Run the Command", "Copy Row", "Find", "Close"]);
        for item in tool.context_items().into_iter().filter(|item| !item.is_separator()) {
            let mut probe = tool_with_rows(3);
            let mut last = ToolOutcome::Ignored;
            for key in &item.keys {
                last = probe.on_key(key);
            }
            assert_ne!(last, ToolOutcome::Ignored, "{}", item.label);
            if item.label == "Copy Row" {
                assert!(matches!(last, ToolOutcome::Yank(_)), "{last:?}");
            }
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
                runs_in: None,
            },
            CommandRow {
                action: "git_page".to_string(),
                chord: "C+S+g".to_string(),
                label: "Git: Status".to_string(),
                runs_in: None,
            },
        ];
        let mut tool = KeysTool::from_rows(commands);
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

    #[test]
    fn test_a_tool_command_is_listed_with_its_chord_and_runs_only_in_its_tool() {
        let mut tool = KeysTool::new(&Keymap::default());
        let row = tool
            .commands
            .iter()
            .position(|row| row.action == "open_remote")
            .expect("the Git tool's command is listed");
        assert_eq!(tool.commands[row].chord, "M-g r");
        tool.buffer.move_to(row, 0);
        assert_eq!(tool.on_key(&press(KeyCode::Enter)), ToolOutcome::Consumed);
        assert_eq!(
            tool.take_notice().map(|notice| notice.text).as_deref(),
            Some("runs in a Git tab")
        );
    }

    #[test]
    fn test_every_key_of_the_tool_is_spelled_right() {
        assert_eq!(table::invalid_specs(KEYS), Vec::<&str>::new());
    }
}
