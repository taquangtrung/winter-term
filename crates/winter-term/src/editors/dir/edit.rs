//! Editing the listed names in place, in the manner of Emacs' `wdired`, and
//! the renames an edit session comes to.

use crate::model::editor::{EditorOutcome, Notice};
use crate::model::input::table::{self, TableMatch};
use crate::model::input::{Key, KeyCode};
use crate::model::textedit::field::TextField;
use crate::model::textedit::modal::EditOutcome;

use super::keys::{Act, EDIT_KEYS};
use super::tree::Row;
use super::{name_of, ops, rows, DirEditor, ASK_APPLY_EDITS};

// ========================================================================
// Constants
// ========================================================================

/// How many renames a batch's confirmation names before it just counts.
const RENAMES_LISTED: usize = 3;

// ========================================================================
// DirEditor: editing the names
// ========================================================================

impl DirEditor {
    /// Turn every listed name into editable text, the caret parked at the end
    /// of the name under the cursor, where a rename usually begins.
    pub(super) fn enter_edit(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        let names = self.rows.iter().map(|row| row.entry.name.clone()).collect();
        self.edit = Some(TextField::rows(names, self.list.cursor()));
    }

    /// Keys while the names are editable text. `C-x C-q` toggles the
    /// session off from either mode, and `q` with nothing part-typed leaves
    /// it as it leaves the listing; everything else goes to the modal keys,
    /// whose lines are the listing's rows, so the cursor follows the line.
    /// `ZZ`, `C-s`, or `Enter` while typing applies the renames, and `ZQ`
    /// throws them away.
    pub(super) fn on_edit_key(&mut self, key: &Key) -> EditorOutcome {
        if let Some(first) = self.pending.take() {
            if table::lookup(EDIT_KEYS, &[first, *key]) == TableMatch::Exact(Act::Edit) {
                self.discard_edit();
            }
            return EditorOutcome::Consumed;
        }
        if table::lookup(EDIT_KEYS, std::slice::from_ref(key)) == TableMatch::Prefix {
            self.pending = Some(*key);
            return EditorOutcome::Consumed;
        }
        let Some(edit) = self.edit.as_mut() else {
            return EditorOutcome::Ignored;
        };
        if edit.modal().is_idle() && table::find(EDIT_KEYS, key) == Some(Act::Discard) {
            self.discard_edit();
            return EditorOutcome::Consumed;
        }
        edit.move_to_row(self.list.cursor());
        let outcome = edit.on_key(key, self.list.view().rows().max(1));
        self.list.set_cursor(edit.row());
        if let Some(notice) = edit.take_notice() {
            self.notice = Some(notice);
        }
        match outcome {
            EditOutcome::Quit { save: true } | EditOutcome::Save | EditOutcome::Submit => {
                self.apply_edits()
            }
            EditOutcome::Quit { save: false } | EditOutcome::Cancel => {
                self.discard_edit();
                EditorOutcome::Consumed
            }
            EditOutcome::Yank(text) => EditorOutcome::Yank(text),
            EditOutcome::Paste => EditorOutcome::Paste,
            // A second `Esc` in Normal is inert rather than the window's, so
            // mashing it cannot close the listing over unapplied edits.
            EditOutcome::Ignored if key.glyph() == KeyCode::Escape => EditorOutcome::Consumed,
            EditOutcome::Ignored => EditorOutcome::Ignored,
            // The listing has no other files, searches, or fuller editor for
            // the names, so these are spent with nothing to do.
            EditOutcome::Consumed
            | EditOutcome::Escalate
            | EditOutcome::ListBuffers
            | EditOutcome::Search { .. }
            | EditOutcome::StepBuffer { .. } => EditorOutcome::Consumed,
        }
    }

    /// Leave edit mode without renaming anything.
    pub(super) fn discard_edit(&mut self) {
        let pending = self.pending_edits().len();
        self.edit = None;
        // Names snapping back to the disk's can read as the edits having been
        // applied; saying they were not is one line of insurance.
        self.notice = (pending > 0).then(|| Notice::info(format!("discarded {pending}")));
    }

    /// Apply the edited names as renames. Every name, and every collision,
    /// is checked before any rename runs, so one bad name cannot leave the
    /// batch half-applied; a swap or a cycle of names goes through. One rename
    /// runs at once; a batch of several is listed for a yes first, since
    /// renaming a column of names is easy to get subtly wrong.
    pub(super) fn apply_edits(&mut self) -> EditorOutcome {
        let pending = self.pending_edits();
        if pending.is_empty() {
            self.edit = None;
            self.notice = Some(Notice::info("no changes"));
            return EditorOutcome::Consumed;
        }
        let mut renames: Vec<ops::Rename> = Vec::new();
        for (index, name) in &pending {
            let from = self.rows[*index].entry.path.clone();
            let Some(to) = from
                .parent()
                .and_then(|dir| ops::resolve_name(dir, name.as_str()))
            else {
                // Stay in edit mode: the name is still on screen, still
                // editable, and now flagged as the problem.
                self.notice = Some(Notice::error(format!("failed: not a name: {name}")));
                return EditorOutcome::Consumed;
            };
            renames.push(ops::Rename {
                depth: self.rows[*index].depth,
                from,
                to,
            });
        }
        if let Err(problem) = ops::check_renames(&renames) {
            self.notice = Some(Notice::error(format!("failed: {problem}")));
            return EditorOutcome::Consumed;
        }
        if renames.len() == 1 {
            return self.run_renames(renames);
        }
        let label = format!(
            "Rename {} entries: {}? (y/n) ",
            renames.len(),
            rename_summary(&renames)
        );
        self.rename_plan = Some(renames);
        self.confirm(ASK_APPLY_EDITS, label)
    }

    /// Carry out a checked batch of renames and leave edit mode.
    pub(super) fn run_renames(&mut self, renames: Vec<ops::Rename>) -> EditorOutcome {
        self.edit = None;
        let (done, failure) = ops::rename_all(&renames);
        self.notice = Some(match failure {
            Some(report) => Notice::error(format!("failed: {report}")),
            None => Notice::info(format!("renamed {done}")),
        });
        self.marks.clear();
        self.reload();
        EditorOutcome::Consumed
    }

    /// The rows whose edited name no longer matches the entry on disk.
    pub(super) fn pending_edits(&self) -> Vec<(usize, String)> {
        let Some(edit) = self.edit.as_ref() else {
            return Vec::new();
        };
        self.rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                let name = edit.lines().get(index)?;
                (*name != row.entry.name).then(|| (index, name.clone()))
            })
            .collect()
    }

    /// How `index`'s name is drawn while editing: the edited text, a caret on
    /// the cursor's row, and whether the edit has drifted from the disk.
    pub(super) fn edited_name(&self, index: usize, row: &Row) -> Option<rows::EditedName> {
        let edit = self.edit.as_ref()?;
        let text = edit.lines().get(index)?.clone();
        Some(rows::EditedName {
            caret: (index == self.list.cursor()).then(|| edit.col()),
            changed: text != row.entry.name,
            text,
        })
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A batch of renames as a confirmation names it: the first few as
/// `old -> new`, and how many more there are.
fn rename_summary(renames: &[ops::Rename]) -> String {
    let mut listed: Vec<String> = renames
        .iter()
        .take(RENAMES_LISTED)
        .map(|rename| format!("{} -> {}", name_of(&rename.from), name_of(&rename.to)))
        .collect();
    if renames.len() > RENAMES_LISTED {
        listed.push(format!("{} more", renames.len() - RENAMES_LISTED));
    }
    listed.join(", ")
}
