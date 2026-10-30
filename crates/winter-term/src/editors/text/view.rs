//! Small things done to the view and the place in the file: `C-l` puts the
//! cursor's line in the middle of the pane, then at its top, then at its
//! bottom, as Emacs' recenter does, and `zz`, `zt`, and `zb`, which the modal
//! keys ask for, put it at one; each counts screen rows, so with wrapping on
//! it is the cursor's screen row that goes there; the cursor's line flashes
//! after a jump,
//! so the eye finds it; and the cursor goes to a character offset from the
//! file's start, or says the one it is at.

use std::time::{Duration, Instant};

use crate::model::editor::{EditorOutcome, Notice, PromptMode, PromptRequest};
use crate::model::paint::{follow_screen_row, screen_rows_above, Place, Shown};

use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// How long the cursor's line stays lit when it flashes.
const FLASH_FOR: Duration = Duration::from_millis(400);

/// The question an offset to go to answers.
pub(super) const ASK_OFFSET: &str = "goto-offset";

// ========================================================================
// Data Structures
// ========================================================================

/// Where a run of `C-l` presses has got to: the line it is for, and which
/// place the line was put at last.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Recenter {
    row: usize,
    step: Place,
}

// ========================================================================
// TextEditor: the view
// ========================================================================

impl TextEditor {
    /// Put the cursor's line in the middle of the pane; pressed again on
    /// the same line, at its top, then at its bottom, and round.
    pub(super) fn recenter(&mut self) -> EditorOutcome {
        let row = self.buffer().row();
        let step = match self.recenter {
            Some(Recenter { row: last, step }) if last == row => match step {
                Place::Middle => Place::Top,
                Place::Top => Place::Bottom,
                Place::Bottom => Place::Middle,
            },
            _ => Place::Middle,
        };
        self.place_cursor_row(step);
        self.recenter = Some(Recenter { row, step });
        EditorOutcome::Consumed
    }

    /// Start the view so the cursor's screen row is at `place` in the pane.
    pub(super) fn place_cursor_row(&mut self, place: Place) {
        self.put_cursor_rows_down(place.rows_above(self.viewport, self.scroll_margin));
    }

    /// Start the view `rows` screen rows above the cursor's.
    pub(super) fn put_cursor_rows_down(&mut self, rows: usize) {
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let (within, _, _) = self.segment_at(row, col);
        let (scroll, skip) = screen_rows_above((row, within), rows, |index| self.segments(index).len());
        self.doc_mut().scroll = scroll;
        self.doc_mut().scroll_skip = skip;
    }

    /// Where the view starts: a line, and how many of its screen rows are
    /// scrolled past.
    pub(super) fn view_top(&self) -> (usize, usize) {
        (self.doc().scroll, self.doc().scroll_skip)
    }

    /// The lines on the top, middle, and bottom screen rows of the last
    /// paint, inside the margins, which `H`, `M`, and `L` go to.
    pub(super) fn shown(&self) -> Shown {
        self.laid_out.shown
    }

    /// Whether the cursor's screen row shows in the view as it stands.
    pub(super) fn shows_cursor(&self) -> bool {
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let (within, _, _) = self.segment_at(row, col);
        let top = self.view_top();
        follow_screen_row(top, (row, within), self.viewport, 0, |index| self.segments(index).len()) == top
    }

    /// Light the cursor's line for a moment.
    pub(super) fn flash_line(&mut self) -> EditorOutcome {
        self.flash_until = Some(Instant::now() + FLASH_FOR);
        EditorOutcome::Consumed
    }

    /// Whether the cursor's line is lit now.
    pub(super) fn is_flashing(&self) -> bool {
        self.flash_until.is_some_and(|until| Instant::now() < until)
    }

    /// Ask for the character offset to go to.
    pub(super) fn ask_offset(&mut self) -> EditorOutcome {
        EditorOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: format!("Go to offset (0-{}): ", self.offset_of_end()),
            mode: PromptMode::Text,
            tag: ASK_OFFSET,
        })
    }

    /// Go to character offset `answer` from the file's start, a line end
    /// counting as one.
    pub(super) fn goto_offset(&mut self, answer: &str) -> EditorOutcome {
        let Ok(mut left) = answer.trim().parse::<usize>() else {
            self.modal.report(Notice::info("an offset is a number").unlogged());
            return EditorOutcome::Consumed;
        };
        let lines = self.buffer().lines().to_vec();
        let mut place = (lines.len().saturating_sub(1), lines.last().map_or(0, |line| line.chars().count()));
        for (row, line) in lines.iter().enumerate() {
            let length = line.chars().count();
            if left <= length {
                place = (row, left);
                break;
            }
            left -= length + 1;
        }
        self.doc_mut().marks.jump = Some((self.buffer().row(), self.buffer().col()));
        self.buffer_mut().move_to(place.0, place.1);
        EditorOutcome::Consumed
    }

    /// Say the character offset the cursor is at.
    pub(super) fn show_offset(&mut self) -> EditorOutcome {
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let before: usize = self.buffer().lines()[..row].iter().map(|line| line.chars().count() + 1).sum();
        self.modal.report(Notice::info(format!("offset {}", before + col)).unlogged());
        EditorOutcome::Consumed
    }

    /// The offset just past the file's last character.
    fn offset_of_end(&self) -> usize {
        let lines = self.buffer().lines();
        lines.iter().map(|line| line.chars().count() + 1).sum::<usize>().saturating_sub(1)
    }
}
