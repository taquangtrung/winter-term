//! Local history: a copy of a file as each save wrote it, kept apart from
//! git in the state directory, so text lost past the undo history (after a
//! restart, an edit made elsewhere, a commit not yet made) can be had back.
//! `File: Local History…` lists a file's copies, newest first; the one chosen
//! opens beside the buffer in the diff tool, which takes back what is wanted.
//!
//! Each file keeps its copies in a directory of its own, named for its path,
//! one file a copy named for when it was written. A save that wrote what the
//! newest copy holds keeps none, and the oldest go past [`MAX_COPIES`].

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use winter_model::editor::{EditorOutcome, Notice, PickPlacement, PickRequest};
use winter_model::paint::IconKind;

use super::branches::said_ago;
use super::{name_of, TextEditor};

// ========================================================================
// Constants
// ========================================================================

/// The state directory's folder the copies are kept in.
const HISTORY_DIR: &str = "history";

/// The most copies one file keeps: enough for a day of saving, few enough
/// that a file saved all day costs little.
const MAX_COPIES: usize = 50;

/// The largest file copied, past which a save keeps no copy: a log or a
/// data file saved often would fill the disk.
const MAX_BYTES: usize = 2 * 1024 * 1024;

/// What a copy's file is named after its time, and the file holding the
/// path the copies are of, for a reader of the folder.
const COPY_EXTENSION: &str = "txt";
const PATH_FILE: &str = "path";

/// The question the list of copies answers.
pub(super) const ASK_HISTORY: &str = "file_history";

/// What the list is headed with, and each copy wears.
const HISTORY_LABEL: &str = "Local History";
const HISTORY_ICON: IconKind = IconKind::Symbol { name: "history" };

/// What is said where a file has no copy.
const NO_HISTORY: &str = "no saved copy of this file yet";

// ========================================================================
// Data Structures
// ========================================================================

/// One copy of a file: when it was written, and where it is kept.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Copy {
    at: SystemTime,
    file: PathBuf,
}

// ========================================================================
// Functions
// ========================================================================

/// The folder under `root` the copies of `path` are kept in.
fn folder_of(root: &Path, path: &Path) -> PathBuf {
    root.join(format!("{:016x}", fnv1a(path.to_string_lossy().as_bytes())))
}

/// The 64-bit FNV-1a hash of `bytes`: the same on every run and every
/// build, as a folder name has to be, which the standard hasher is not.
fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
    })
}

/// Keep `text` as a copy of `path` written `at`, under `root`, unless it is
/// what the newest copy holds or too large to keep, and let the oldest go.
pub(super) fn record(root: &Path, path: &Path, text: &str, at: SystemTime) -> io::Result<()> {
    if text.len() > MAX_BYTES {
        return Ok(());
    }
    let copies = copies(root, path);
    if let Some(newest) = copies.first() {
        if std::fs::read_to_string(&newest.file).is_ok_and(|kept| kept == text) {
            return Ok(());
        }
    }
    let folder = folder_of(root, path);
    std::fs::create_dir_all(&folder)?;
    std::fs::write(folder.join(PATH_FILE), path.to_string_lossy().as_bytes())?;
    let millis = at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    std::fs::write(folder.join(format!("{millis}.{COPY_EXTENSION}")), text)?;
    for old in copies.iter().skip(MAX_COPIES - 1) {
        std::fs::remove_file(&old.file)?;
    }
    Ok(())
}

/// The copies of `path` kept under `root`, newest first.
pub(super) fn copies(root: &Path, path: &Path) -> Vec<Copy> {
    let Ok(entries) = std::fs::read_dir(folder_of(root, path)) else {
        return Vec::new();
    };
    let mut found: Vec<Copy> = entries
        .flatten()
        .filter_map(|entry| {
            let file = entry.path();
            if file.extension()? != COPY_EXTENSION {
                return None;
            }
            let millis: u64 = file.file_stem()?.to_str()?.parse().ok()?;
            Some(Copy {
                at: UNIX_EPOCH + Duration::from_millis(millis),
                file,
            })
        })
        .collect();
    found.sort_by(|a, b| b.at.cmp(&a.at));
    found
}

/// Where the window keeps the copies: the state directory's own folder.
pub fn history_dir() -> Option<PathBuf> {
    winter_base::paths::state_file(HISTORY_DIR)
}

/// What the list says of the copy at `at`: its place, how long ago it was
/// written, and how many lines it holds.
fn label_of(at: usize, copy: &Copy, now: SystemTime) -> String {
    let ago = now.duration_since(copy.at).map_or(0, |ago| ago.as_secs());
    let lines = std::fs::read_to_string(&copy.file).map_or(0, |text| text.lines().count());
    format!("{}. saved {}, {lines} lines", at + 1, said_ago(ago))
}

// ========================================================================
// TextEditor: local history
// ========================================================================

impl TextEditor {
    /// Keep a copy of what the save just wrote, for a file on this machine.
    pub(super) fn remember_written(&self) {
        let path = &self.doc().path;
        if winter_base::remote::parse(path).is_some() {
            return;
        }
        let Some(root) = self.history_dir.as_deref() else {
            return;
        };
        let mut text = self.buffer().lines().join("\n");
        text.push('\n');
        // A copy not kept costs the history one entry, never the save.
        let _ = record(root, path, &text, SystemTime::now());
    }

    /// Offer the file's copies, newest first.
    pub(super) fn ask_history(&mut self) -> EditorOutcome {
        let copies = self
            .history_dir
            .as_deref()
            .map_or_else(Vec::new, |root| copies(root, &self.doc().path));
        if copies.is_empty() {
            self.modal.report(Notice::info(NO_HISTORY).unlogged());
            return EditorOutcome::Consumed;
        }
        let now = SystemTime::now();
        let items: Vec<String> = copies
            .iter()
            .enumerate()
            .map(|(at, copy)| label_of(at, copy, now))
            .collect();
        EditorOutcome::Pick(PickRequest {
            icons: vec![HISTORY_ICON; items.len()],
            items,
            label: HISTORY_LABEL.to_string(),
            placeholder: String::new(),
            placement: PickPlacement::Cursor,
            tag: ASK_HISTORY,
        })
    }

    /// Open the copy the list came back with, named by its label, beside
    /// the buffer in the diff tool.
    pub(super) fn history_chosen(&mut self, chosen: &str) -> EditorOutcome {
        let index = chosen
            .split_once('.')
            .and_then(|(number, _)| number.parse::<usize>().ok())
            .and_then(|number| number.checked_sub(1));
        let path = self.doc().path.clone();
        let copy = index.and_then(|index| {
            let root = self.history_dir.as_deref()?;
            copies(root, &path).into_iter().nth(index)
        });
        let Some(copy) = copy else {
            return EditorOutcome::Consumed;
        };
        let text = match std::fs::read_to_string(&copy.file) {
            Ok(text) => text,
            Err(e) => {
                self.modal
                    .report(Notice::error(format!("local history: {e}")));
                return EditorOutcome::Consumed;
            }
        };
        let ago = SystemTime::now()
            .duration_since(copy.at)
            .map_or(0, |ago| ago.as_secs());
        let label = format!("{} (saved {})", name_of(&path), said_ago(ago));
        self.compare_with_lines(label, text.lines().map(str::to_string).collect())
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn root(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("winter-history-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn test_each_save_keeps_a_copy_newest_first_and_an_unchanged_one_none() {
        let root = root("record");
        let path = Path::new("/work/notes.md");
        record(&root, path, "one\n", at(10)).unwrap();
        record(&root, path, "two\n", at(20)).unwrap();
        record(&root, path, "two\n", at(30)).unwrap();
        let kept = copies(&root, path);
        assert_eq!(kept.len(), 2, "the unchanged save kept no copy");
        assert_eq!(kept[0].at, at(20));
        assert_eq!(std::fs::read_to_string(&kept[1].file).unwrap(), "one\n");
        assert!(copies(&root, Path::new("/work/other.md")).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_the_oldest_copies_go_past_the_most_kept() {
        let root = root("prune");
        let path = Path::new("/work/a.rs");
        for n in 0..MAX_COPIES as u64 + 5 {
            record(&root, path, &format!("{n}\n"), at(n + 1)).unwrap();
        }
        let kept = copies(&root, path);
        assert_eq!(kept.len(), MAX_COPIES);
        assert_eq!(kept[0].at, at(MAX_COPIES as u64 + 5), "the newest stays");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_a_save_keeps_a_copy_and_choosing_it_compares_it_with_the_buffer() {
        use winter_model::editor::Page;
        use winter_model::input::{Key, KeyCode};

        let root = root("editor");
        let dir = root.join("files");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.txt");
        std::fs::write(&path, "ab\n").unwrap();
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        editor.history_dir = Some(root.join("kept"));
        editor.on_key(&Key::plain(KeyCode::Char('x')));
        editor.save();
        let kept = copies(&root.join("kept"), &path);
        assert_eq!(kept.len(), 1);
        assert_eq!(std::fs::read_to_string(&kept[0].file).unwrap(), "b\n");
        assert!(matches!(editor.ask_history(), EditorOutcome::Pick(_)));
        assert!(matches!(
            editor.history_chosen("1. saved just now, 1 lines"),
            EditorOutcome::Diff(_)
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_a_file_too_large_keeps_no_copy() {
        let root = root("large");
        let path = Path::new("/work/big.log");
        record(&root, path, &"x".repeat(MAX_BYTES + 1), at(1)).unwrap();
        assert!(copies(&root, path).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
