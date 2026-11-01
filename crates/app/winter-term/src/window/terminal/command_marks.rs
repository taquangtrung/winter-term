//! What a terminal shows of its commands beside its grid: each command's exit
//! status in the gutter, on the row its prompt began, and, where the user asks
//! for it, the command whose output fills the top of the pane pinned over its
//! first row once that command's prompt is scrolled out of view.

use winter_emulator::CommandBlock;
use winter_model::layout::TabId;
use winter_model::paint::{GutterLane, GutterMark, Span, Style};
use winter_terminal::session::Terminal;

use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// The marks of a command that exited zero, one that exited non-zero, and
/// one the shell started and has not reported done: a glyph each, one
/// column wide, so the gutter takes no more of the terminal than it must.
const SUCCEEDED: &str = "✓";
const FAILED: &str = "✗";
const RUNNING: &str = "…";

/// How wide the gutter's one lane is, in columns.
const LANE_WIDTH: usize = 1;

// ========================================================================
// App
// ========================================================================

impl App {
    /// Scroll terminal `tab` to the prompt of the command its sticky header
    /// names, the answer to a press on the header.
    pub(in crate::window) fn jump_to_sticky_prompt(&mut self, tab: TabId) {
        let Some(terminal) = self.open_tabs.terminal_mut(&tab) else {
            return;
        };
        let Some(start) = sticky_block(terminal).map(|index| terminal.block_rows()[index]) else {
            return;
        };
        terminal.grid_mut().scroll_to_absolute_row(start);
        self.dirty = true;
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// The lanes of every terminal's gutter: one column for the status marks,
/// kept whether or not the shell reports any, so the PTY's width never
/// changes when the first one arrives.
pub(in crate::window) fn gutter_lanes() -> Vec<GutterLane> {
    vec![GutterLane::Text { width: LANE_WIDTH }]
}

/// The gutter marks of each of `terminal`'s viewport rows: a command's status
/// on the row its prompt began, nothing on every other row, and nothing at all
/// while a full-screen program owns the grid.
pub(in crate::window) fn gutter_marks(terminal: &Terminal) -> Vec<Vec<Option<GutterMark>>> {
    let grid = terminal.grid();
    let mut marks = vec![Vec::new(); grid.rows()];
    if grid.is_alt_screen() {
        return marks;
    }
    let blocks = terminal.scrollback().blocks();
    let last = blocks.len().saturating_sub(1);
    for (index, (block, &start)) in blocks.iter().zip(terminal.block_rows()).enumerate() {
        let Some(kind) = status_mark(block, index == last) else {
            continue;
        };
        let row = usize::try_from(grid.to_viewport_row(start)).ok();
        if let Some(slot) = row.and_then(|row| marks.get_mut(row)) {
            *slot = vec![Some(kind)];
        }
    }
    marks
}

/// The index of the command `terminal`'s sticky header names: the one whose
/// output is on the viewport's top row while its prompt is above it. `None`
/// while that prompt is in view, above the first command, or while a
/// full-screen program owns the grid.
pub(in crate::window) fn sticky_block(terminal: &Terminal) -> Option<usize> {
    let grid = terminal.grid();
    if grid.is_alt_screen() {
        return None;
    }
    let top = grid.to_absolute_row(0);
    let starts = terminal.block_rows();
    let index = starts.iter().rposition(|&start| start < top)?;
    // A later block starting on the top row itself has its prompt in view.
    let later_in_view = starts.get(index + 1).is_some_and(|&start| start <= top);
    let block = terminal.scrollback().blocks().get(index)?;
    (!later_in_view && !block.command.is_empty()).then_some(index)
}

/// The mark of `block`, the newest of its terminal's when `last`: its exit
/// status once the shell reported one, running while the newest has a
/// command and none yet. A block with no command (a prompt still waiting,
/// or a shell without integration) has none.
fn status_mark(block: &CommandBlock, last: bool) -> Option<GutterMark> {
    if block.command.is_empty() {
        return None;
    }
    let (style, glyph) = match block.exit_code {
        Some(0) => (Style::Passed, SUCCEEDED),
        Some(_) => (Style::Failed, FAILED),
        None if last => (Style::Dim, RUNNING),
        None => return None,
    };
    Some(GutterMark::Text(Span::new(style, glyph)))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const COLS: usize = 40;

    const ROWS: usize = 6;

    /// A terminal over `cat`, its program output faked as `bytes`.
    fn terminal_with(bytes: &[u8]) -> Terminal {
        let mut terminal = Terminal::with_command(
            COLS,
            ROWS,
            portable_pty::CommandBuilder::new("cat"),
            winter_emulator::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        terminal.feed_program_output(bytes);
        terminal
    }

    /// The shell-integration bytes of `command` writing `output` and exiting
    /// `code`, then the next prompt waiting.
    fn finished(command: &str, output: &str, code: i32) -> String {
        format!(
            "\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\{command}\x1b]133;C\x1b\\\r\n{output}\r\n\x1b]133;D;{code}\x1b\\\x1b]133;A\x1b\\$ "
        )
    }

    /// The mark in `marks`' row `row`.
    fn mark_at(marks: &[Vec<Option<GutterMark>>], row: usize) -> Option<&GutterMark> {
        marks[row].first().and_then(Option::as_ref)
    }

    #[test]
    fn test_each_finished_command_wears_its_exit_status_on_its_prompt_row() {
        let bytes = finished("true", "ok", 0)
            + "\x1b]133;B\x1b\\false\x1b]133;C\x1b\\\r\n\x1b]133;D;1\x1b\\\x1b]133;A\x1b\\$ ";
        let terminal = terminal_with(bytes.as_bytes());
        let marks = gutter_marks(&terminal);
        assert_eq!(
            mark_at(&marks, 0),
            Some(&GutterMark::Text(Span::new(Style::Passed, SUCCEEDED)))
        );
        assert_eq!(mark_at(&marks, 1), None, "the output row");
        assert_eq!(
            mark_at(&marks, 2),
            Some(&GutterMark::Text(Span::new(Style::Failed, FAILED)))
        );
        assert_eq!(
            mark_at(&marks, 3),
            None,
            "the prompt still waiting has no command"
        );
    }

    #[test]
    fn test_a_command_with_no_exit_yet_is_running() {
        let terminal =
            terminal_with(b"\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\sleep 9\x1b]133;C\x1b\\\r\n");
        let marks = gutter_marks(&terminal);
        assert_eq!(
            mark_at(&marks, 0),
            Some(&GutterMark::Text(Span::new(Style::Dim, RUNNING)))
        );
    }

    #[test]
    fn test_a_full_screen_program_hides_every_mark() {
        let bytes = finished("true", "ok", 0) + "\x1b[?1049h";
        let terminal = terminal_with(bytes.as_bytes());
        assert!(gutter_marks(&terminal).iter().all(Vec::is_empty));
    }

    #[test]
    fn test_the_sticky_header_waits_for_the_prompt_to_scroll_off() {
        let short = terminal_with(finished("ls", "a", 0).as_bytes());
        assert_eq!(sticky_block(&short), None, "the prompt is on screen");

        let output = (0..ROWS * 2)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>();
        let long = terminal_with(finished("ls", &output.join("\r\n"), 0).as_bytes());
        assert_eq!(
            sticky_block(&long),
            Some(0),
            "the output of `ls` fills the top row"
        );
    }
}
