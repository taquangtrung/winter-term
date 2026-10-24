//! Reading the filesystem into listing rows. The only part of Dir that touches
//! the disk.
//!
//! A read is planned by the listing ([`ReadPlan`]) and carried out off the
//! window's thread ([`read_listing`]), to a budget of entries and with a flag
//! that stops it part-way: a directory of a million files, a slow network
//! mount, or a whole home directory opened at once must never hold the frame,
//! or read without end.

use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

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

/// How many entries one read takes in before it stops, however much is
/// open: more rows than anyone scrolls through, few enough to read quickly.
pub const MAX_ENTRIES: usize = 50_000;

// ========================================================================
// Data Structures
// ========================================================================

/// What a read of the listing covers: the directory, how it is shown, which
/// directories are open, and what to open before reading.
#[derive(Clone, Debug)]
pub struct ReadPlan {
    /// The directories open before the read.
    pub folds: Folds,
    /// What to open first.
    pub opening: Opening,
    /// The directory listed.
    pub root: PathBuf,
    /// Whether dotfiles are listed.
    pub show_hidden: bool,
    /// The order entries are listed in.
    pub sort: SortKey,
}

/// What a read opens before it reads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Opening {
    /// Nothing: the directories open are the ones the folds already hold.
    AsFolded,
    /// Every directory down to this many levels below the root, the rest
    /// shut.
    Depth(usize),
    /// This directory and every directory under it.
    Subtree(PathBuf),
}

/// What a read found: the rows, the folds they were read with (the plan's,
/// plus whatever it opened), and whether it stopped short of all of it.
#[derive(Clone, Debug)]
pub struct Listing {
    /// The directories open, as the rows show them.
    pub folds: Folds,
    /// The rows, in order.
    pub rows: Vec<Row>,
    /// Whether the read ran out of budget, or was stopped, before the end.
    pub truncated: bool,
}

/// What a read may still take in, and the flag that stops it.
struct Budget<'a> {
    /// Set when the read is no longer wanted.
    cancel: &'a AtomicBool,
    /// Entries still to be read.
    left: usize,
}

// ========================================================================
// Budget
// ========================================================================

impl<'a> Budget<'a> {
    fn new(left: usize, cancel: &'a AtomicBool) -> Self {
        Self { cancel, left }
    }

    /// Whether the read must stop: out of entries, or no longer wanted.
    fn is_spent(&self) -> bool {
        self.left == 0 || self.cancel.load(Ordering::Relaxed)
    }

    /// Take up to `count` entries from what is left, saying how many.
    fn take(&mut self, count: usize) -> usize {
        let taken = count.min(self.left);
        self.left -= taken;
        taken
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Carry out `plan` to [`MAX_ENTRIES`], or until `cancel` is set.
pub fn read_listing(plan: ReadPlan, cancel: &AtomicBool) -> Listing {
    read_listing_within(plan, MAX_ENTRIES, cancel)
}

/// Carry out `plan`, reading at most `limit` entries. A directory that cannot
/// be read contributes no rows: a listing shows what it can rather than
/// failing whole.
fn read_listing_within(plan: ReadPlan, limit: usize, cancel: &AtomicBool) -> Listing {
    let ReadPlan {
        mut folds,
        opening,
        root,
        show_hidden,
        sort,
    } = plan;
    let mut budget = Budget::new(limit, cancel);
    match opening {
        Opening::AsFolded => {}
        Opening::Subtree(dir) => {
            let mut found = Vec::new();
            collect_dirs(&dir, show_hidden, 0, &mut budget, &mut found);
            for path in found {
                folds.expand(&path);
            }
            folds.expand(&dir);
            // The walk spent what it read; the read itself starts afresh.
            budget = Budget::new(limit, cancel);
        }
        Opening::Depth(levels) => {
            folds.collapse_all();
            // One level at a time, so the whole tree is never read at once.
            for _ in 0..levels {
                let rows = read_rows_at(&root, 0, show_hidden, sort, &folds, &mut budget);
                let shut: Vec<PathBuf> = rows
                    .into_iter()
                    .filter(|row| row.entry.is_dir() && !row.expanded)
                    .map(|row| row.entry.path)
                    .collect();
                if shut.is_empty() || budget.is_spent() {
                    break;
                }
                for path in shut {
                    folds.expand(&path);
                }
                budget = Budget::new(limit, cancel);
            }
        }
    }
    let rows = read_rows_at(&root, 0, show_hidden, sort, &folds, &mut budget);
    Listing {
        folds,
        rows,
        truncated: budget.is_spent(),
    }
}

fn read_rows_at(
    dir: &Path,
    depth: usize,
    show_hidden: bool,
    sort: SortKey,
    folds: &Folds,
    budget: &mut Budget,
) -> Vec<Row> {
    if depth >= MAX_DEPTH || budget.is_spent() {
        return Vec::new();
    }
    let mut entries = read_entries(dir, show_hidden, budget);
    sort_entries(&mut entries, sort);

    let mut rows = Vec::with_capacity(entries.len());
    for entry in entries {
        let expanded = entry.is_dir() && folds.is_expanded(&entry.path);
        let path = entry.path.clone();
        rows.push(Row::new(depth, entry, expanded));
        if expanded {
            rows.extend(read_rows_at(
                &path,
                depth + 1,
                show_hidden,
                sort,
                folds,
                budget,
            ));
        }
    }
    rows
}

/// Every directory under `dir`, to the reader's own depth cap and to the
/// budget, so expanding a subtree cannot read without limit.
fn collect_dirs(
    dir: &Path,
    show_hidden: bool,
    depth: usize,
    budget: &mut Budget,
    found: &mut Vec<PathBuf>,
) {
    if depth >= MAX_DEPTH || budget.is_spent() {
        return;
    }
    for entry in read_entries(dir, show_hidden, budget) {
        if !entry.is_dir() {
            continue;
        }
        collect_dirs(&entry.path, show_hidden, depth + 1, budget, found);
        found.push(entry.path);
    }
}

/// Every entry of `dir`, unsorted, as far as the budget goes. The budget is
/// checked as the directory is read rather than after, so one enormous
/// directory stops the read in the middle of itself.
fn read_entries(dir: &Path, show_hidden: bool, budget: &mut Budget) -> Vec<Entry> {
    let normalized = crate::model::path::normalize_path(dir);
    let Ok(reader) = fs::read_dir(&normalized) else {
        return Vec::new();
    };
    reader
        .flatten()
        .take_while(|_| budget.take(1) == 1 && !budget.cancel.load(Ordering::Relaxed))
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

    /// A plan reading `root` as `folds` open it.
    fn plan(root: &Path, show_hidden: bool, folds: &Folds, opening: Opening) -> ReadPlan {
        ReadPlan {
            folds: folds.clone(),
            opening,
            root: root.to_path_buf(),
            show_hidden,
            sort: SortKey::Name,
        }
    }

    fn read_rows(root: &Path, show_hidden: bool, _sort: SortKey, folds: &Folds) -> Vec<Row> {
        read_listing(
            plan(root, show_hidden, folds, Opening::AsFolded),
            &AtomicBool::new(false),
        )
        .rows
    }

    #[test]
    fn test_a_read_stops_at_its_budget_and_says_it_did() {
        let tree = TempTree::new("budget");
        for n in 0..10 {
            tree.touch(&format!("f{n}"));
        }
        let cut = read_listing_within(
            plan(&tree.0, false, &Folds::new(), Opening::AsFolded),
            4,
            &AtomicBool::new(false),
        );
        assert_eq!(cut.rows.len(), 4);
        assert!(cut.truncated);
        let whole = read_listing_within(
            plan(&tree.0, false, &Folds::new(), Opening::AsFolded),
            100,
            &AtomicBool::new(false),
        );
        assert_eq!(whole.rows.len(), 10);
        assert!(!whole.truncated);
    }

    #[test]
    fn test_a_stopped_read_returns_what_it_had() {
        let tree = TempTree::new("stopped");
        tree.touch("a");
        let stopped = read_listing(
            plan(&tree.0, false, &Folds::new(), Opening::AsFolded),
            &AtomicBool::new(true),
        );
        assert!(stopped.rows.is_empty());
        assert!(stopped.truncated);
    }

    #[test]
    fn test_opening_a_subtree_opens_every_directory_under_it() {
        let tree = TempTree::new("subtree");
        let deep = tree.dir("a").join("b").join("c");
        fs::create_dir_all(&deep).expect("deep");
        fs::write(deep.join("leaf"), "x").expect("leaf");
        let listing = read_listing(
            plan(
                &tree.0,
                false,
                &Folds::new(),
                Opening::Subtree(tree.0.join("a")),
            ),
            &AtomicBool::new(false),
        );
        let names: Vec<&str> = listing
            .rows
            .iter()
            .map(|row| row.entry.name.as_str())
            .collect();
        assert_eq!(names, ["a", "b", "c", "leaf"]);
        assert!(listing.folds.is_expanded(&deep));
    }

    #[test]
    fn test_opening_to_a_depth_shuts_what_lies_deeper() {
        let tree = TempTree::new("depth");
        fs::create_dir_all(tree.0.join("a").join("b").join("c")).expect("deep");
        let mut folds = Folds::new();
        folds.expand(&tree.0.join("a").join("b").join("c"));
        let listing = read_listing(
            plan(&tree.0, false, &folds, Opening::Depth(1)),
            &AtomicBool::new(false),
        );
        let names: Vec<&str> = listing
            .rows
            .iter()
            .map(|row| row.entry.name.as_str())
            .collect();
        assert_eq!(names, ["a", "b"], "one level open, b shut");
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
