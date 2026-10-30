//! Buffers: one file's text and what goes with it, held once however many
//! tabs show it. Each tab keeps a view of the text, with a cursor, a scroll,
//! and colors of its own; the text, its undo history, what was last saved,
//! where the file stood on disk, and what a language server has of it are
//! the buffer's, so an edit in one tab is the others' at once, a save in one
//! is a save in all, and a server hears of the file once.
//!
//! The buffers open are kept by where their file is, so a second tab asking
//! for a file joins the buffer the first one read rather than reading it again.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use crate::editors::text_file::{self, FileShape, FileStamp};
use crate::model::textedit::buffer::{SharedText, TextBuffer};

use super::lsp::Synced;
use super::{Document, TextEditor};

// ========================================================================
// Data Structures
// ========================================================================

/// What every tab showing one file holds in common beside the text itself.
#[derive(Debug)]
pub(super) struct BufferState {
    /// How many tabs on it are open with a server, so the last to go is the
    /// one that tells the server the file is closed.
    attached: Cell<usize>,
    /// The text as Copilot last had it.
    pilot_synced: Cell<Option<Synced>>,
    shape: Cell<FileShape>,
    /// Where the file stood on disk when it was last read or written.
    stamp: RefCell<FileStamp>,
    /// The text as the language server serving the file last had it.
    synced: Cell<Option<Synced>>,
    /// The text, for a tab joining it later to open a view of.
    text: SharedText,
}

thread_local! {
    /// Every buffer read in, by where its file is, for a tab asking for the
    /// file to join. A buffer every tab has let go of is gone from here too.
    static BUFFERS: RefCell<HashMap<PathBuf, Weak<BufferState>>> = RefCell::new(HashMap::new());
}

// ========================================================================
// BufferState
// ========================================================================

impl BufferState {
    /// How the file's lines are laid out, which a write keeps.
    pub(super) fn shape(&self) -> FileShape {
        self.shape.get()
    }

    /// What goes with `buffer`'s text, its file shaped `shape` and standing
    /// at `stamp` on disk.
    pub(super) fn new(buffer: &TextBuffer, shape: FileShape, stamp: FileStamp) -> Rc<Self> {
        Rc::new(Self {
            attached: Cell::new(0),
            pilot_synced: Cell::new(None),
            shape: Cell::new(shape),
            stamp: RefCell::new(stamp),
            synced: Cell::new(None),
            text: buffer.shared_text(),
        })
    }
}

// ========================================================================
// Document: the buffer's part
// ========================================================================

impl Document {
    /// Where the file stood on disk when it was last read or written.
    pub(super) fn stamp(&self) -> FileStamp {
        self.file.stamp.borrow().clone()
    }

    pub(super) fn set_stamp(&self, stamp: FileStamp) {
        *self.file.stamp.borrow_mut() = stamp;
    }

    pub(super) fn shape(&self) -> FileShape {
        self.file.shape.get()
    }

    pub(super) fn set_shape(&self, shape: FileShape) {
        self.file.shape.set(shape);
    }

    /// The text as the language server serving the file last had it.
    pub(super) fn synced(&self) -> Option<Synced> {
        self.file.synced.get()
    }

    /// Take `synced` as what the server has, this tab counting as one open
    /// with it from the first time it has anything.
    pub(super) fn set_synced(&mut self, synced: Option<Synced>) {
        self.file.synced.set(synced);
        self.attach(synced.is_some());
    }

    /// The text as Copilot last had it.
    pub(super) fn pilot_synced(&self) -> Option<Synced> {
        self.file.pilot_synced.get()
    }

    pub(super) fn set_pilot_synced(&mut self, synced: Option<Synced>) {
        self.file.pilot_synced.set(synced);
        self.attach(synced.is_some());
    }

    /// Count this tab as open with the servers, the first time `opened`.
    fn attach(&mut self, opened: bool) {
        if opened && !self.attached {
            self.attached = true;
            self.file.attached.set(self.file.attached.get() + 1);
        }
    }

    /// Stop counting this tab as open with the servers. What the servers
    /// have of the file, to tell them it is closed, where this tab was the
    /// last open with them; nothing while another tab still is.
    pub(super) fn detach(&mut self) -> Vec<Synced> {
        if !std::mem::take(&mut self.attached) {
            return Vec::new();
        }
        let left = self.file.attached.get().saturating_sub(1);
        self.file.attached.set(left);
        if left > 0 {
            return Vec::new();
        }
        [self.file.synced.take(), self.file.pilot_synced.take()]
            .into_iter()
            .flatten()
            .collect()
    }

}

// ========================================================================
// TextEditor: keeping up with the other tabs
// ========================================================================

impl TextEditor {
    /// Take in what other tabs changed in the buffers this one shows, ahead
    /// of anything that reads or changes them.
    pub(super) fn catch_up_buffers(&mut self) {
        for doc in &mut self.docs {
            doc.buffer.catch_up();
        }
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// The buffer a tab already has `path` open in, for another to join.
pub(super) fn open_buffer(path: &Path) -> Option<Rc<BufferState>> {
    let key = key_of(path);
    BUFFERS.with(|buffers| buffers.borrow().get(&key).and_then(Weak::upgrade))
}

/// Keep `file` as `path`'s buffer, for tabs asking for the file later.
pub(super) fn keep_buffer(path: &Path, file: &Rc<BufferState>) {
    let key = key_of(path);
    BUFFERS.with(|buffers| {
        let mut buffers = buffers.borrow_mut();
        // The ones every tab has let go of go too.
        buffers.retain(|_, file| file.strong_count() > 0);
        buffers.insert(key, Rc::downgrade(file));
    });
}

/// Write `lines` to the file at `path` through the buffer a tab has it open
/// in, laid out as that buffer's file is: every tab on it then shows what
/// was written, clean, and none keeps an older copy a save of its own could
/// put back over it. `None` where no tab has the file open, for the caller
/// to write it itself.
pub(crate) fn save_through(path: &Path, lines: &[String]) -> Option<io::Result<FileStamp>> {
    let file = open_buffer(path)?;
    Some(text_file::save(path, lines, file.shape()).inspect(|stamp| {
        view_of(&file).replace_all(lines.to_vec());
        *file.stamp.borrow_mut() = stamp.clone();
    }))
}

/// Every file open as text in the window, by where it really is, each with a
/// view of its text as it stands, edits included.
pub(super) fn open_files() -> Vec<(PathBuf, TextBuffer)> {
    BUFFERS.with(|buffers| {
        let mut open: Vec<(PathBuf, TextBuffer)> = buffers
            .borrow()
            .iter()
            .filter_map(|(path, file)| Some((path.clone(), view_of(file.upgrade()?.as_ref()))))
            .collect();
        open.sort_by(|a, b| a.0.cmp(&b.0));
        open
    })
}

/// A view of `file`'s text, for a tab joining the buffer.
pub(super) fn view_of(file: &BufferState) -> TextBuffer {
    file.text.view()
}

/// What a file is known by: where it really is, through every symlink,
/// where it is on this machine; its address as it stands elsewhere.
pub(super) fn key_of(path: &Path) -> PathBuf {
    match crate::remote::parse(path) {
        Some(_) => path.to_path_buf(),
        None => path.canonicalize().unwrap_or_else(|_| path.to_path_buf()),
    }
}
