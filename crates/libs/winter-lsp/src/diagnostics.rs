//! What is wrong in the files, gathered from every back-end that checks
//! them into one store, as VS Code's diagnostics and Emacs's flycheck
//! gather them: the editors underline, count, and step through what is
//! here without knowing which back-end found it.
//!
//! A back-end publishes a file's problems whenever it has checked the file
//! again, the new list replacing what it said of the file before; an empty
//! list says the file is clean. A new back-end is a [`Backend`] variant and
//! the code that publishes under it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use crate::protocol::Diagnostic;

// ========================================================================
// Constants
// ========================================================================

/// The one store, made the first time it is used.
static STORE: OnceLock<Mutex<Store>> = OnceLock::new();

/// Moves whenever the store changes, so a reader holding a copy of what it
/// found knows to take it again.
static GENERATION: AtomicU64 = AtomicU64::new(0);

// ========================================================================
// Data Structures
// ========================================================================

/// What checks the files, each publishing under its own name so one never
/// replaces what another found.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Backend {
    /// An extension, by its id.
    Extension(&'static str),
    /// A language server, by the id the window started it under: two
    /// servers on one file, or on nested projects, each keep their own.
    Lsp(u64),
    /// The spelling of the prose.
    Spelling,
}

/// One problem, the file it is in, and the back-end that found it.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// What found it.
    pub backend: Backend,
    /// The problem.
    pub diagnostic: Diagnostic,
    /// The file it is in.
    pub path: PathBuf,
}

/// Each back-end's problems, file by file.
type Store = BTreeMap<Backend, BTreeMap<PathBuf, Vec<Diagnostic>>>;

// ========================================================================
// Backend
// ========================================================================

impl Backend {
    /// Whether what it finds is about the writing rather than the code.
    pub fn is_writing(self) -> bool {
        matches!(self, Self::Spelling)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Take `diagnostics` as everything `backend` finds wrong in `path` now;
/// none says the file is clean.
pub fn publish(backend: Backend, path: &Path, diagnostics: Vec<Diagnostic>) {
    let key = key_of(path);
    let mut store = store();
    let files = store.entry(backend).or_default();
    match diagnostics.is_empty() {
        true => files.remove(&key),
        false => files.insert(key, diagnostics),
    };
    changed();
}

/// Forget everything `backend` found: its checker has stopped.
pub fn forget(backend: Backend) {
    store().remove(&backend);
    changed();
}

/// Forget what `backend` found in files under `root` that are no longer on
/// disk: deleted or moved since it last checked them.
pub fn forget_gone(backend: Backend, root: &Path) {
    let root = key_of(root);
    if let Some(files) = store().get_mut(&backend) {
        files.retain(|path, _| !path.starts_with(&root) || path.exists());
    }
    changed();
}

/// How many times the store has changed: a copy taken at an earlier count
/// is out of date.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// Note that the store has changed.
fn changed() {
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// What `backend` found in `path`.
pub fn of(backend: Backend, path: &Path) -> Vec<Diagnostic> {
    store()
        .get(&backend)
        .and_then(|files| files.get(&key_of(path)))
        .cloned()
        .unwrap_or_default()
}

/// What every back-end found in `path`, in position order, named as the
/// caller named it.
pub fn in_file(path: &Path) -> Vec<Found> {
    let key = key_of(path);
    let mut found: Vec<Found> = store()
        .iter()
        .filter_map(|(backend, files)| Some((*backend, files.get(&key)?)))
        .flat_map(|(backend, diagnostics)| diagnostics.iter().map(move |d| found(backend, path, d)))
        .collect();
    found.sort_by_key(|f| f.diagnostic.range.start);
    found
}

/// What every back-end found in the files under `root`, in file and then
/// position order.
pub fn under(root: &Path) -> Vec<Found> {
    let root = key_of(root);
    let root = root.as_path();
    let mut found: Vec<Found> = store()
        .iter()
        .flat_map(|(backend, files)| {
            files
                .iter()
                .filter(|(path, _)| path.starts_with(root))
                .flat_map(move |(path, diagnostics)| {
                    diagnostics.iter().map(move |d| found(*backend, path, d))
                })
        })
        .collect();
    found.sort_by(|a, b| {
        (&a.path, a.diagnostic.range.start).cmp(&(&b.path, b.diagnostic.range.start))
    });
    found
}

/// What a file is kept under: where it really is, through every symlink, so
/// one opened through a link and reported by a server that resolves links
/// is the same file. A path that cannot be resolved (a remote one, a file
/// since deleted) is kept as it is.
fn key_of(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// `diagnostic`, which `backend` found in `path`.
fn found(backend: Backend, path: &Path, diagnostic: &Diagnostic) -> Found {
    Found {
        backend,
        diagnostic: diagnostic.clone(),
        path: path.to_path_buf(),
    }
}

/// The store, locked; one a panic left poisoned is still read.
fn store() -> MutexGuard<'static, Store> {
    STORE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use crate::protocol::{Position, Range, Severity};

    /// A problem at `line`, column 0.
    fn at_line(line: u32) -> Diagnostic {
        let start = Position { character: 0, line };
        Diagnostic {
            code: None,
            message: format!("line {line}"),
            range: Range { end: start, start },
            severity: Severity::Error,
            source: None,
        }
    }

    #[test]
    fn test_a_backend_replaces_only_its_own_and_an_empty_list_clears() {
        let path = Path::new("/diagnostics-test/replace/a.md");
        publish(Backend::Lsp(101), path, vec![at_line(3)]);
        publish(Backend::Spelling, path, vec![at_line(1)]);
        publish(Backend::Spelling, path, vec![at_line(2)]);
        let lines: Vec<u32> = in_file(path)
            .iter()
            .map(|f| f.diagnostic.range.start.line)
            .collect();
        assert_eq!(
            lines,
            [2, 3],
            "spelling replaced, the server's kept, in order"
        );
        publish(Backend::Spelling, path, Vec::new());
        assert_eq!(in_file(path).len(), 1);
        assert_eq!(of(Backend::Lsp(101), path), [at_line(3)]);
    }

    #[test]
    fn test_under_keeps_to_the_root_in_file_then_position_order() {
        let root = Path::new("/diagnostics-test/under");
        publish(Backend::Spelling, &root.join("b.md"), vec![at_line(0)]);
        publish(
            Backend::Lsp(102),
            &root.join("a.rs"),
            vec![at_line(5), at_line(1)],
        );
        publish(
            Backend::Lsp(102),
            Path::new("/diagnostics-test/elsewhere.rs"),
            vec![at_line(0)],
        );
        let order: Vec<(String, u32)> = under(root)
            .iter()
            .map(|f| (f.path.display().to_string(), f.diagnostic.range.start.line))
            .collect();
        let a = root.join("a.rs").display().to_string();
        let b = root.join("b.md").display().to_string();
        assert_eq!(order, [(a.clone(), 1), (a, 5), (b, 0)]);
        forget(Backend::Lsp(102));
        assert_eq!(under(root).len(), 1, "the spelling stays");
    }

    #[test]
    fn test_stopping_one_server_keeps_what_another_said_about_the_file() {
        // Keyed by "the language servers" as one, stopping either of two
        // servers on a file wiped the other's problems too.
        let path = Path::new("/diagnostics-test/two-servers.rs");
        publish(Backend::Lsp(103), path, vec![at_line(1)]);
        publish(Backend::Lsp(104), path, vec![at_line(2)]);
        forget(Backend::Lsp(103));
        assert!(of(Backend::Lsp(103), path).is_empty());
        assert_eq!(of(Backend::Lsp(104), path), [at_line(2)]);
    }

    #[test]
    fn test_forget_gone_drops_only_the_files_no_longer_on_disk() {
        let root =
            std::env::temp_dir().join(format!("winter-diagnostics-gone-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("dir");
        let kept = root.join("kept.md");
        std::fs::write(&kept, "x").expect("file");
        publish(Backend::Spelling, &kept, vec![at_line(0)]);
        publish(
            Backend::Spelling,
            &root.join("deleted.md"),
            vec![at_line(0)],
        );
        forget_gone(Backend::Spelling, &root);
        let left: Vec<PathBuf> = under(&root).into_iter().map(|f| f.path).collect();
        assert_eq!(left, [kept]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
