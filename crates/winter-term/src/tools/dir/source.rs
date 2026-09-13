//! Reading the filesystem into listing rows. The only part of Dir that touches
//! the disk.

use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};

use super::entry::{Entry, EntryKind, Meta};
use super::listing::{is_hidden, sort_entries, SortKey};
use super::tree::{Folds, Row};

// ========================================================================
// Constants
// ========================================================================

/// How deep an expanded listing is allowed to nest. Symlinks are never
/// followed, so this is a guard against a pathological tree rather than a
/// cycle, and it keeps one keystroke from reading an unbounded subtree.
const MAX_DEPTH: usize = 32;

// ========================================================================
// Functions
// ========================================================================

/// Read `dir` into sorted rows, descending into whatever `folds` has expanded.
/// A directory that cannot be read contributes no rows: a listing shows what it
/// can rather than failing whole.
pub fn read_rows(dir: &Path, show_hidden: bool, sort: SortKey, folds: &Folds) -> Vec<Row> {
    read_rows_at(dir, 0, show_hidden, sort, folds)
}

fn read_rows_at(
    dir: &Path,
    depth: usize,
    show_hidden: bool,
    sort: SortKey,
    folds: &Folds,
) -> Vec<Row> {
    if depth >= MAX_DEPTH {
        return Vec::new();
    }
    let mut entries = read_entries(dir, show_hidden);
    sort_entries(&mut entries, sort);

    let mut rows = Vec::with_capacity(entries.len());
    for entry in entries {
        let expanded = entry.is_dir() && folds.is_expanded(&entry.path);
        let path = entry.path.clone();
        rows.push(Row::new(depth, entry, expanded));
        if expanded {
            rows.extend(read_rows_at(&path, depth + 1, show_hidden, sort, folds));
        }
    }
    rows
}

/// Every directory under `dir`, to the reader's own depth cap, so expanding a
/// subtree cannot read without limit.
pub fn descendant_dirs(dir: &Path, show_hidden: bool) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect_dirs(dir, show_hidden, 0, &mut found);
    found
}

fn collect_dirs(dir: &Path, show_hidden: bool, depth: usize, found: &mut Vec<PathBuf>) {
    if depth >= MAX_DEPTH {
        return;
    }
    for entry in read_entries(dir, show_hidden) {
        if !entry.is_dir() {
            continue;
        }
        collect_dirs(&entry.path, show_hidden, depth + 1, found);
        found.push(entry.path);
    }
}

/// Every entry of `dir`, unsorted.
fn read_entries(dir: &Path, show_hidden: bool) -> Vec<Entry> {
    let Ok(reader) = fs::read_dir(dir) else {
        return Vec::new();
    };
    reader
        .flatten()
        .filter_map(|dir_entry| {
            let name = dir_entry.file_name().to_string_lossy().to_string();
            if !show_hidden && is_hidden(&name) {
                return None;
            }
            // Deliberately not following the link: a symlinked directory is
            // shown as a link, so an expanded listing cannot walk a cycle.
            let metadata = fs::symlink_metadata(dir_entry.path()).ok();
            Some(Entry::new(
                kind_of(metadata.as_ref()),
                meta_of(metadata.as_ref()),
                name,
                dir_entry.path(),
            ))
        })
        .collect()
}

fn kind_of(metadata: Option<&Metadata>) -> EntryKind {
    match metadata {
        Some(meta) if meta.file_type().is_symlink() => EntryKind::Symlink,
        Some(meta) if meta.is_dir() => EntryKind::Dir,
        Some(_) | None => EntryKind::File,
    }
}

fn meta_of(metadata: Option<&Metadata>) -> Meta {
    let Some(meta) = metadata else {
        return Meta::default();
    };
    Meta {
        len: meta.len(),
        mode: mode_of(meta),
        modified: meta.modified().ok(),
    }
}

#[cfg(unix)]
fn mode_of(metadata: &Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.mode())
}

#[cfg(not(unix))]
fn mode_of(_metadata: &Metadata) -> Option<u32> {
    None
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary tree that removes itself on drop.
    struct TempTree(PathBuf);

    impl TempTree {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("winter-dir-test-{tag}-{}", std::process::id()));
            fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }

        fn dir(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::create_dir_all(&path).expect("temp subdir");
            path
        }

        fn touch(&self, name: &str) {
            fs::write(self.0.join(name), "x").expect("temp file");
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn test_hidden_entries_appear_only_when_asked_for() {
        let tree = TempTree::new("hidden");
        tree.touch("visible.txt");
        tree.touch(".hidden");

        let shown = read_rows(&tree.0, false, SortKey::Name, &Folds::new());
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].entry.name, "visible.txt");

        let all = read_rows(&tree.0, true, SortKey::Name, &Folds::new());
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_an_expanded_directory_lists_its_children_one_level_deeper() {
        let tree = TempTree::new("expand");
        let nested = tree.dir("nested");
        fs::write(nested.join("inner.txt"), "x").expect("inner file");

        let mut folds = Folds::new();
        folds.toggle(&nested);
        let rows = read_rows(&tree.0, false, SortKey::Name, &folds);

        assert_eq!(rows.len(), 2, "the directory and its one child");
        assert_eq!(rows[0].depth, 0);
        assert!(rows[0].expanded);
        assert_eq!(rows[1].entry.name, "inner.txt");
        assert_eq!(rows[1].depth, 1);
    }

    #[test]
    fn test_an_unreadable_directory_contributes_no_rows() {
        // A listing must survive a directory that disappeared or was never
        // readable, rather than losing the rows around it.
        let rows = read_rows(
            Path::new("/definitely/not/a/directory"),
            false,
            SortKey::Name,
            &Folds::new(),
        );
        assert!(rows.is_empty());
    }
}
