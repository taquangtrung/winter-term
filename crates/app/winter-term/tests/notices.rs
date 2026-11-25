//! Holds every notice's wording to one place: a misty `Notice` is built
//! from text only in a file named `notice.rs`, where an enum's `match` turns
//! each variant into its words and severity.
//!
//! Everywhere else, code reports a variant and tests compare variants, so a
//! message is reworded in one line and no test or caller needs to follow.
//! Text from another program (git's errors, a language server's) travels as
//! a variant's data, never as a notice built where it was read.
//!
//! The only exceptions are the files in [`NOTICE_DATA_FILES`], whose tests
//! post sample notices to the pipeline that carries them.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

// ========================================================================
// Constants
// ========================================================================

/// The one file name a notice may be worded in.
const NOTICE_FILE: &str = "notice.rs";

/// This file, which names the constructors to look for them. A path under
/// `crates/`.
const THIS_FILE: &str = "app/winter-term/tests/notices.rs";

/// The constructors that put words in a notice.
const CONSTRUCTORS: [&str; 4] = [
    "Notice::info(",
    "Notice::refusal(",
    "Notice::error(",
    "Notice::warning(",
];

/// Files that hold notices as data rather than wording them, each a part of
/// the pipeline every notice goes through, whose tests post samples to it:
/// the window's notifier, the Notifications page that lists what was
/// posted, and a test editor that posts one. Paths under `crates/`.
const NOTICE_DATA_FILES: &[&str] = &[
    "app/winter-term/src/editors/notifications.rs",
    "app/winter-term/src/window/editor/tests/opening.rs",
    "app/winter-term/src/window/notices.rs",
];

// ========================================================================
// Functions
// ========================================================================

/// The `crates/` directory.
fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the crates directory")
}

/// Every `.rs` file under `dir` but those named [`NOTICE_FILE`].
fn sources(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for path in fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
        {
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && path.file_name().is_some_and(|name| name != NOTICE_FILE)
            {
                found.push(path);
            }
        }
    }
    found
}

/// The lines of `text` that build a notice from words, comments left out.
fn worded_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .filter(|line| CONSTRUCTORS.iter().any(|built| line.contains(built)))
        .collect()
}

// ========================================================================
// Tests
// ========================================================================

#[test]
fn test_every_notice_is_worded_in_a_notice_module() {
    let root = crates_dir();
    let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in sources(&root) {
        let text = fs::read_to_string(&path).unwrap_or_default();
        let lines = worded_lines(&text);
        let file = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if lines.is_empty() || file == THIS_FILE || NOTICE_DATA_FILES.contains(&file.as_str()) {
            continue;
        }
        found.insert(
            file,
            lines.iter().map(|line| line.trim().to_string()).collect(),
        );
    }
    let wrong: Vec<String> = found
        .iter()
        .map(|(file, lines)| {
            format!(
                "{file}: {} notice(s) worded here; add a variant to the crate's notice enum \
                 instead:\n  {}",
                lines.len(),
                lines.join("\n  ")
            )
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn test_a_comment_naming_a_constructor_is_no_notice() {
    let text = "// Notice::info(\"said\")\n    let n = Notice::refusal(\"no\");\n";
    assert_eq!(worded_lines(text), ["    let n = Notice::refusal(\"no\");"]);
}
