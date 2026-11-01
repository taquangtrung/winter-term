//! Next edits: the edit a source expects after a change elsewhere, shown in
//! a card under the cursor. `Tab` makes it, `Esc` (on its way back to
//! Normal mode) and `C-g` leave it.

use winter_lsp::protocol::{self, Range, TextEdit};
use winter_model::editor::{EditorOutcome, InfoCard};
use winter_model::input::{Key, KeyCode, Mods};

use super::{end_of, Edit, OfferKind};
use crate::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// The most lines a next edit's card shows of each side.
const CARD_LINES: usize = 8;

// ========================================================================
// Data Structures
// ========================================================================

/// A next edit on offer: the buffer's revision it was offered at (an edit
/// since makes it stale), and the source it came from.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Pending {
    pub(super) edit: Edit,
    pub(super) revision: usize,
    /// The source's place in the editor's list.
    pub(super) source: usize,
}

// ========================================================================
// TextEditor: next edits
// ========================================================================

impl TextEditor {
    /// Offer the next edit the source at `at` expects, and tell it so.
    pub(super) fn offer_edit(&mut self, at: usize, edit: Edit) {
        let receipt = edit.receipt.clone();
        self.inline.edit = Some(Pending {
            edit,
            revision: self.buffer().revision(),
            source: at,
        });
        self.with_source(at, |source, editor| {
            source.shown(editor, OfferKind::Edit, &receipt)
        });
    }

    /// A key a next edit on offer gives a meaning of its own.
    pub(super) fn next_edit_key(&mut self, key: &Key) -> Option<EditorOutcome> {
        match key.chord() {
            (Mods::NONE, KeyCode::Tab) => Some(self.take_next_edit()),
            (Mods::NONE, KeyCode::Escape) => {
                self.inline.edit = None;
                None
            }
            (Mods::CTRL, KeyCode::Char('g')) => {
                self.inline.edit = None;
                Some(EditorOutcome::Consumed)
            }
            _ => None,
        }
    }

    /// Make the next edit on offer, the cursor after it, where the text it
    /// was offered for is still there.
    fn take_next_edit(&mut self) -> EditorOutcome {
        let Some(pending) = self.inline.edit.take() else {
            return EditorOutcome::Consumed;
        };
        if pending.revision != self.buffer().revision() {
            return EditorOutcome::Consumed;
        }
        let edit = &pending.edit;
        let lines = self.buffer().lines();
        let range = Range {
            end: protocol::position_in(lines, edit.end.0, edit.end.1),
            start: protocol::position_in(lines, edit.start.0, edit.start.1),
        };
        let at = self.at.min(self.docs.len() - 1);
        self.apply_to_buffer(
            at,
            &[TextEdit {
                new_text: edit.text.clone(),
                range,
            }],
        );
        let (end_row, end_col) = end_of(edit.start.0, edit.start.1, &edit.text);
        match self.modal.is_typing() {
            true => self.buffer_mut().move_to_typing(end_row, end_col),
            false => self
                .buffer_mut()
                .move_to(end_row, end_col.saturating_sub(1)),
        }
        self.tell_taken(pending.source, OfferKind::Edit, &pending.edit.receipt);
        EditorOutcome::Consumed
    }

    /// The card a next edit shows in: what it takes out and puts in.
    pub(super) fn next_edit_card(&self) -> Option<InfoCard> {
        let pending = self.inline.edit.as_ref()?;
        let edit = &pending.edit;
        let lines = self.buffer().lines();
        let ((start_row, start_col), (end_row, end_col)) = (edit.start, edit.end);
        let taken: Vec<String> = match start_row == end_row {
            true => vec![lines
                .get(start_row)?
                .chars()
                .skip(start_col)
                .take(end_col.saturating_sub(start_col))
                .collect()],
            false => lines
                .get(start_row..=end_row.min(lines.len().saturating_sub(1)))?
                .to_vec(),
        };
        let name = self.source_name(pending.source);
        let mut text = format!(
            "{name}: line {} (Tab to make it, Esc to leave it)",
            start_row + 1
        );
        for line in taken
            .iter()
            .filter(|line| !line.is_empty())
            .take(CARD_LINES)
        {
            text.push_str(&format!("\n- {line}"));
        }
        for line in edit.text.lines().take(CARD_LINES) {
            text.push_str(&format!("\n+ {line}"));
        }
        Some(InfoCard {
            cell: None,
            footer: None,
            text,
        })
    }
}
