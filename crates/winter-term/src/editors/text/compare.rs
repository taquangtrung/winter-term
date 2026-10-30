//! Comparing what a buffer holds with something else: the file on disk it was
//! read from, the clipboard, or another open buffer, each as it stands, in
//! the diff tool, where the buffer's side is written back to its file.

use crate::editors::text_file;
use crate::model::editor::{DiffRequest, DiffText, EditorOutcome, Notice, PickPlacement, PickRequest};

use super::buffers;
use super::{name_of, TextEditor, ASK_COMPARE_BUFFER};

// ========================================================================
// TextEditor: comparing
// ========================================================================

impl TextEditor {
    /// Compare the buffer with the file on disk it was read from: what the
    /// edits not yet saved would change.
    pub(super) fn compare_with_disk(&mut self) -> EditorOutcome {
        let path = self.doc().path.clone();
        if crate::remote::parse(&path).is_some() || !path.is_file() {
            self.modal
                .report(Notice::error("compare: no file on this machine to compare with"));
            return EditorOutcome::Consumed;
        }
        let disk = match text_file::load(&path) {
            Ok(loaded) => loaded.lines,
            Err(e) => {
                self.modal.report(Notice::error(format!("compare: {e}")));
                return EditorOutcome::Consumed;
            }
        };
        let label = format!("{} (on disk)", name_of(&path));
        compare(text(label, disk), self.side_of(self.at))
    }

    /// Ask the host for the clipboard, to compare the buffer with it once it
    /// comes back (see [`Self::compare_with_clipboard`]).
    pub(super) fn ask_to_compare_with_clipboard(&mut self) -> EditorOutcome {
        self.compare_clipboard = true;
        EditorOutcome::Paste
    }

    /// Compare the buffer with `text`, the clipboard: what pasting it over
    /// the buffer would change.
    pub(super) fn compare_with_clipboard(&mut self, clipboard: &str) -> EditorOutcome {
        let lines = clipboard.lines().map(str::to_string).collect();
        compare(text("clipboard".to_string(), lines), self.side_of(self.at))
    }

    /// The other files open as text in the window, to choose one to compare
    /// this buffer with.
    pub(super) fn ask_buffer_to_compare(&mut self) -> EditorOutcome {
        let own = buffers::key_of(&self.doc().path);
        let items: Vec<String> = buffers::open_files()
            .into_iter()
            .filter(|(path, _)| *path != own)
            .map(|(path, _)| path.to_string_lossy().into_owned())
            .collect();
        if items.is_empty() {
            self.modal
                .report(Notice::error("compare: only this file is open"));
            return EditorOutcome::Consumed;
        }
        EditorOutcome::Pick(PickRequest {
            icons: Vec::new(),
            items,
            label: "Compare this buffer with".to_string(),
            placement: PickPlacement::Centered,
            tag: ASK_COMPARE_BUFFER,
        })
    }

    /// Compare the buffer with the open file whose path is `chosen`, each as
    /// it stands, edits included.
    pub(super) fn compare_with_buffer(&mut self, chosen: &str) -> EditorOutcome {
        let other = buffers::open_files()
            .into_iter()
            .find(|(path, _)| path.to_string_lossy() == chosen);
        let Some((path, buffer)) = other else {
            return EditorOutcome::Consumed;
        };
        let edited = if buffer.is_dirty() { " (edited)" } else { "" };
        let side = DiffText {
            label: format!("{}{edited}", crate::model::path::abbreviate_home(&path)),
            lines: buffer.lines().to_vec(),
            shape: buffers::open_buffer(&path).map(|file| file.shape()),
            path: Some(path),
        };
        compare(side, self.side_of(self.at))
    }

    /// Buffer `at` as a side of a comparison, written back to its file.
    pub(super) fn side_of(&self, at: usize) -> DiffText {
        let doc = &self.docs[at];
        let edited = if doc.buffer.is_dirty() { " (edited)" } else { "" };
        DiffText {
            label: format!("{}{edited}", crate::model::path::abbreviate_home(&doc.path)),
            lines: doc.buffer.lines().to_vec(),
            path: Some(doc.path.clone()),
            shape: Some(doc.shape()),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A side no file stands behind: the disk's copy, the clipboard.
fn text(label: String, lines: Vec<String>) -> DiffText {
    DiffText {
        label,
        lines,
        path: None,
        shape: None,
    }
}

/// `old` and `new` compared in the diff tool.
fn compare(old: DiffText, new: DiffText) -> EditorOutcome {
    EditorOutcome::Diff(DiffRequest {
        ancestor: None,
        merge: None,
        texts: vec![old, new],
    })
}
