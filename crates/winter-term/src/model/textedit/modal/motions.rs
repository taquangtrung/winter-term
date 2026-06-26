//! Searching, and moving the cursor by motions.

use crate::model::textedit::search::Search;
use crate::model::tool::Notice;
use crate::model::vim::motion::CursorMove;
use crate::model::vim::objects::word_object;

use super::{move_cursor, EditOutcome, Session, HALF_PAGE};

// ========================================================================
// Session: search
// ========================================================================

impl Session<'_> {
    /// The next match of the last search (`n`), or the one the other way
    /// (`N`).
    pub(super) fn search_step(&mut self, reverse: bool) -> EditOutcome {
        let Some(search) = self.ed.search.clone() else {
            self.ed.notice = Some(Notice::error("nothing has been searched for"));
            return EditOutcome::Consumed;
        };
        self.ed.searched = true;
        let forward = search.forward != reverse;
        let at = (self.buffer().row(), self.buffer().col());
        match search.next(self.buffer().lines(), at, forward) {
            Some((row, col)) => {
                self.push_jump();
                self.buffer_mut().move_to(row, col);
            }
            None => {
                self.ed.notice = Some(Notice::error(format!(
                    "`{}` is not in the file",
                    search.pattern
                )))
            }
        }
        EditOutcome::Consumed
    }

    /// Look for the word under the cursor (`*`/`#`).
    pub(super) fn search_word(&mut self, forward: bool) -> EditOutcome {
        let line = self.buffer().chars();
        let Some((start, end)) = word_object(&line, self.buffer().col(), false, false) else {
            return EditOutcome::Consumed;
        };
        let word: String = line[start..=end].iter().collect();
        if word.trim().is_empty() {
            return EditOutcome::Consumed;
        }
        self.ed.search = Some(Search::new(word, forward));
        self.search_step(false)
    }
}

// ========================================================================
// Session: motions
// ========================================================================

impl Session<'_> {
    /// Move the cursor as `motion` says, in the terms the view gives them.
    pub(super) fn apply_motion(&mut self, motion: CursorMove, typed: Option<usize>) {
        let row = self.buffer().row();
        let viewport = self.view.viewport;
        match motion {
            // The scroll placements move the window rather than the cursor,
            // so they are the view's to answer, not the text's.
            CursorMove::LineToTop => *self.view.scroll = row,
            CursorMove::LineToCenter => {
                *self.view.scroll = row.saturating_sub(viewport / HALF_PAGE)
            }
            CursorMove::LineToBottom => {
                *self.view.scroll = row.saturating_sub(viewport.saturating_sub(1));
            }
            _ => {
                let (tool, scroll) = (viewport.max(1), *self.view.scroll);
                move_cursor(self.view.buffer, motion, typed, tool, scroll);
            }
        }
    }

    /// Remember where a jump started, so `''` can go back to it.
    pub(super) fn push_jump(&mut self) {
        self.view.marks.jump = Some((self.buffer().row(), self.buffer().col()));
    }

    /// Remember the cursor ahead of the motions that carry the eye a long way.
    pub(super) fn jump_for(&mut self, motion: CursorMove) {
        let jumps = matches!(
            motion,
            CursorMove::Bottom
                | CursorMove::MatchingBracket
                | CursorMove::PageDown
                | CursorMove::PageUp
                | CursorMove::ParagraphBack
                | CursorMove::ParagraphForward
                | CursorMove::Top
        );
        if jumps {
            self.push_jump();
        }
    }
}
