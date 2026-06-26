//! What a listing is made of: one entry, what kind of thing it is, and the
//! metadata behind the detail columns.

use std::path::PathBuf;
use std::time::SystemTime;

// ========================================================================
// Data Structures
// ========================================================================

/// One item in a directory listing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    /// What the entry is, which decides whether it can be descended into.
    pub kind: EntryKind,
    /// Metadata behind the detail columns.
    pub meta: Meta,
    /// The entry's own name, with no parent path attached.
    pub name: String,
    /// Full path, so an entry stays resolvable after the listing moves.
    pub path: PathBuf,
}

/// What an entry is. A symlink is its own kind rather than what it points at,
/// so a listing never follows one and never walks a cycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryKind {
    /// A directory.
    Dir,
    /// A regular file, or anything else that is not a directory or a link.
    File,
    /// A symbolic link, whatever it resolves to.
    Symlink,
}

/// Metadata a detail row shows. A piece that could not be read stays `None`
/// rather than being faked: a file can vanish between the listing and the stat.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Meta {
    /// Size in bytes. Meaningless for a directory, which shows no size.
    pub len: u64,
    /// Unix mode bits, absent on platforms without them.
    pub mode: Option<u32>,
    /// Last modification time.
    pub modified: Option<SystemTime>,
}

// ========================================================================
// Entry
// ========================================================================

impl Entry {
    /// An entry at `path`, named `name`.
    pub fn new(kind: EntryKind, meta: Meta, name: String, path: PathBuf) -> Self {
        Self {
            kind,
            meta,
            name,
            path,
        }
    }

    /// Whether this entry can be descended into or expanded.
    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Dir
    }
}
