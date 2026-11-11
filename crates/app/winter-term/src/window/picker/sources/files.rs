//! The file pickers: every file under a directory, found by its path and
//! opened. `C-M-z` lists the focused tab's project; `M-S-z` only what is
//! under the focused tab's own directory.
//!
//! The files are the ones git knows of or would, its ignored ones left out,
//! else what `rg --files` lists, which honours the same ignore files. They
//! are listed on a thread of their own, so the picker opens at once.

use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex, PoisonError};
use std::thread;

use misty::picker::{
    Action, ActionId, Batch, Feed, MatchOn, Matching, PreviewKind, Resume, Row, RowKey, Source,
};
use winter_base::process;
use winter_base::wake::wake_window;
use winter_model::editor::OpenTarget;
use winter_model::paint::IconKind;
use winter_model::path::abbreviate_home;

use crate::window::picker::sources::IconRow;
use crate::window::picker::WindowOutcome;
use crate::window::picker::{FilePreview, Preview};

// ========================================================================
// Constants
// ========================================================================

/// The most files listed: past this the list is too long to read, and the
/// query is what finds a file anyway.
const MAX_FILES: usize = 50_000;

/// How many files go to the picker in one batch.
const BATCH_FILES: usize = 5_000;

/// Said when nothing could list the files.
const NO_FILES: &str = "no files listed: not a git repository, and ripgrep (rg) is not installed";

/// What a buffer of the files listed is called.
const FILES_NAME: &str = "Files";

/// The listing thread's name.
const THREAD_NAME: &str = "picker-files";

/// The actions of a file's row: open it here, or beside, the latter also a
/// button.
pub(in crate::window) const OPEN: ActionId = ActionId("open");
pub(in crate::window) const OPEN_BESIDE: ActionId = ActionId("open_beside");

/// The button's icon for opening beside.
const BESIDE_ICON: IconKind = IconKind::Symbol {
    name: "split-horizontal",
};

// ========================================================================
// Data Structures
// ========================================================================

/// The files under a root.
pub(in crate::window) struct FileSource {
    /// The paths from the root, by row, filled by the listing thread.
    files: Arc<Mutex<Vec<String>>>,
    id: &'static str,
    root: PathBuf,
}

// ========================================================================
// FileSource
// ========================================================================

impl FileSource {
    /// The files of the project at `root`, as `C-M-z` lists them.
    pub(in crate::window) fn project(root: PathBuf) -> Self {
        Self::with("project_files", root)
    }

    /// The files under `root`, as `M-S-z` lists them.
    pub(in crate::window) fn under(root: PathBuf) -> Self {
        Self::with("find_file", root)
    }

    fn with(id: &'static str, root: PathBuf) -> Self {
        Self {
            files: Arc::default(),
            id,
            root,
        }
    }

    fn path(&self, row: RowKey) -> Option<PathBuf> {
        let files = self.files.lock().unwrap_or_else(PoisonError::into_inner);
        let relative = files.get(usize::try_from(row.0).ok()?)?;
        Some(self.root.join(relative))
    }
}

impl Source<WindowOutcome, Preview> for FileSource {
    fn id(&self) -> &'static str {
        self.id
    }

    fn title(&self) -> String {
        format!("Files in {}", abbreviate_home(&self.root))
    }

    fn name(&self) -> String {
        FILES_NAME.to_string()
    }

    fn placeholder(&self) -> String {
        "File name or path".to_string()
    }

    fn matching(&self) -> Matching {
        Matching::Words(MatchOn::LABEL)
    }

    fn resume(&self) -> Resume {
        Resume::Reread
    }

    fn preview_kind(&self) -> PreviewKind {
        PreviewKind::Panel
    }

    fn ranks_recent(&self) -> bool {
        true
    }

    fn recency_name(&self, row: RowKey) -> Option<String> {
        Some(self.path(row)?.to_string_lossy().into_owned())
    }

    /// List the files on a thread of its own, git or ripgrep running there
    /// rather than on the window's, keeping their paths for the rows to name.
    fn feed(&mut self, _query: &str) -> Feed {
        let (send, batches) = mpsc::channel();
        let (root, kept) = (self.root.clone(), Arc::clone(&self.files));
        let spawned = thread::Builder::new()
            .name(THREAD_NAME.to_string())
            .spawn(move || {
                let Some(files) = list_files(&root) else {
                    let _ = send.send(Batch {
                        done: true,
                        error: Some(NO_FILES.to_string()),
                        ..Batch::default()
                    });
                    wake_window();
                    return;
                };
                *kept.lock().unwrap_or_else(PoisonError::into_inner) = files.clone();
                send_files(&files, &send);
            });
        match spawned {
            Ok(_) => Feed::Stream(batches),
            Err(_) => Feed::Failed(NO_FILES.to_string()),
        }
    }

    fn actions(&self, _row: RowKey) -> Vec<Action> {
        vec![
            Action {
                button: None,
                id: OPEN,
                title: "Open".to_string(),
            },
            Action {
                button: crate::icons::name_for(&BESIDE_ICON),
                id: OPEN_BESIDE,
                title: "Open Beside".to_string(),
            },
        ]
    }

    fn run(&mut self, action: ActionId, row: RowKey, _query: &str) -> Option<WindowOutcome> {
        let target = OpenTarget::file(self.path(row)?);
        match action {
            OPEN_BESIDE => Some(WindowOutcome::OpenFileBeside(target)),
            _ => Some(WindowOutcome::OpenFile(target)),
        }
    }

    fn preview(&self, row: RowKey) -> Option<Preview> {
        Some(Preview::File(FilePreview {
            column: None,
            line: 1,
            path: self.path(row)?,
        }))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The rows of `files`, keyed by their places, in batches.
fn send_files(files: &[String], send: &mpsc::Sender<Batch>) {
    let batches = files.chunks(BATCH_FILES).count();
    for (at, chunk) in files.chunks(BATCH_FILES).enumerate() {
        let first = at * BATCH_FILES;
        let rows = chunk
            .iter()
            .enumerate()
            .map(|(n, path)| file_row(RowKey((first + n) as u64), path))
            .collect();
        let batch = Batch {
            done: at + 1 == batches,
            rows,
            ..Batch::default()
        };
        let taken = send.send(batch).is_ok();
        wake_window();
        if !taken {
            return;
        }
    }
    if batches == 0 {
        let _ = send.send(Batch {
            done: true,
            ..Batch::default()
        });
        wake_window();
    }
}

/// A file's row: its path from the root, with its type's icon.
fn file_row(key: RowKey, path: &str) -> Row {
    let name = path.rsplit('/').next().unwrap_or(path).to_string();
    Row::new(key, path).with_icon_kind(IconKind::File { name })
}

/// The files under `root`, as `/`-separated paths from it, sorted: git's
/// list where `root` is a repository, else ripgrep's. `None` when neither
/// could list it.
fn list_files(root: &Path) -> Option<Vec<String>> {
    let git = [
        "ls-files",
        "-z",
        "--cached",
        "--others",
        "--exclude-standard",
    ];
    let rg = ["--files", "--null"];
    let listing = output_in(root, "git", &git).or_else(|| output_in(root, "rg", &rg))?;
    Some(parse_listing(&listing))
}

/// The paths in a NUL-separated listing, once each, sorted and capped.
fn parse_listing(listing: &str) -> Vec<String> {
    let mut files: Vec<String> = listing
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(|path| path.replace('\\', "/"))
        .collect();
    files.sort();
    // A file both tracked and changed is listed twice by `git ls-files`.
    files.dedup();
    files.truncate(MAX_FILES);
    files
}

/// What `program` prints when run in `dir`, when it succeeds.
fn output_in(dir: &Path, program: &str, args: &[&str]) -> Option<String> {
    let output = process::command(program)
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_listing_is_read_once_each_in_path_order() {
        let files = parse_listing("b.rs\0a.rs\0b.rs\0src\\c.rs\0");
        assert_eq!(files, ["a.rs", "b.rs", "src/c.rs"]);
    }

    #[test]
    fn test_this_repository_is_listed_in_batches_ending_done() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut source = FileSource::project(root);
        let Feed::Stream(batches) = source.feed("") else {
            panic!("a stream");
        };
        let batches: Vec<Batch> = batches.iter().collect();
        assert!(batches.last().is_some_and(|batch| batch.done));
        let rows: Vec<&Row> = batches.iter().flat_map(|batch| &batch.rows).collect();
        assert!(rows.iter().any(|row| row.label == "Cargo.toml"));
    }
}
