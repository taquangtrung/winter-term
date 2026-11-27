//! Walking a directory tree on a worker thread: the size of everything under
//! it, the PDFs in it, and the lines holding one of a list of words whole,
//! through ripgrep when it is installed and a plain walk when it is not.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use winter_model::editor::{DirTotal, SearchHit, SearchRequest, SearchResult};

use crate::editors::todo::{Comments, find_todo};

// ========================================================================
// Constants
// ========================================================================

/// Longest a hit's text is kept. One minified line can run to megabytes, which
/// no pane can show and nothing should hold a list of.
const SEARCH_MAX_LINE: usize = 300;

/// Largest file a search reads. Anything past this is generated, packed, or
/// both, and reading it costs more than the match is worth.
const SEARCH_MAX_FILE_BYTES: u64 = 1 << 20;

/// The largest PDF listed for its text to be read: a book of scans is far
/// past what its text is worth waiting for.
const SEARCH_MAX_PDF_BYTES: u64 = 128 << 20;

/// How many levels below the directory it totals a size walk reports the
/// totals of: its children, and theirs, so stepping in twice shows sizes at
/// once.
const SIZE_INNER_LEVELS: usize = 2;

/// How deep a walk goes, matching the size walk's own ceiling.
const SEARCH_MAX_DEPTH: usize = 32;

/// Directories a walk never enters: version-control internals and build
/// output, which are derived rather than written and would swamp every result.
const SEARCH_SKIP_DIRS: [&str; 5] = [".git", ".hg", ".svn", "node_modules", "target"];

/// The program a search hands its walk to where it is installed: it reads the
/// tree's `.gitignore` files, and it is faster than any walk written here.
pub(super) const RIPGREP: &str = "rg";

/// The byte ripgrep is asked to end each path with, since a path may hold the
/// `:` that would otherwise separate it from the line number.
const PATH_END: u8 = 0;

// ========================================================================
// Functions
// ========================================================================

/// Total bytes under `dir`, skipping what cannot be read and links rather than
/// following them, the way `du` does, with the total of each directory down to
/// [`SIZE_INNER_LEVELS`] below it added to `inner`. Returns what it summed so
/// far when cancelled, which the caller discards.
pub(super) fn walk_size(
    dir: &Path,
    cancel: &AtomicBool,
    depth: usize,
    inner: &mut Vec<DirTotal>,
) -> u64 {
    const MAX_DEPTH: usize = 64;
    if depth >= MAX_DEPTH || cancel.load(Ordering::Relaxed) {
        return 0;
    }
    let Ok(reader) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0;
    for entry in reader.flatten() {
        if cancel.load(Ordering::Relaxed) {
            return total;
        }
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if meta.is_dir() && !meta.file_type().is_symlink() {
            let bytes = walk_size(&entry.path(), cancel, depth + 1, inner);
            if depth < SIZE_INNER_LEVELS {
                inner.push(DirTotal {
                    bytes,
                    path: entry.path(),
                });
            }
            total += bytes;
        } else {
            total += meta.len();
        }
    }
    total
}

/// Every line under the requested root holding one of its words whole, in
/// its own case: `TODO` finds `// TODO: x` and `TODO(me)`, never `todos`.
/// No words find nothing rather than everything. A search for todos keeps
/// only the lines holding one, as the Todo list shows them.
pub(super) fn search_tree(request: &SearchRequest, cancel: &AtomicBool) -> SearchResult {
    let mut found = SearchResult::default();
    if request.words.is_empty() {
        return found;
    }
    match search_with_ripgrep(request, cancel) {
        Some(ripgrep) => found = ripgrep,
        None => walk(&request.root, cancel, 0, &mut |path, meta| {
            if meta.len() <= SEARCH_MAX_FILE_BYTES {
                search_file(path, &request.words, &mut found.hits);
            }
        }),
    }
    if request.todos {
        let mut comments = Comments::default();
        found
            .hits
            .retain_mut(|hit| find_todo(hit, &request.words, &mut comments));
    }
    found
}

/// The PDFs under `root` a search reads, in path order, as far as the
/// walk got before `cancel` was set.
pub(crate) fn pdfs_under(root: &Path, cancel: &AtomicBool) -> Vec<PathBuf> {
    let mut pdfs = Vec::new();
    walk(root, cancel, 0, &mut |path, meta| {
        if is_pdf(path) && meta.len() <= SEARCH_MAX_PDF_BYTES {
            pdfs.push(path.to_path_buf());
        }
    });
    pdfs
}

/// The same search run by ripgrep, under the same rules as the walk: the
/// words found whole in their own case, hidden files read, the same
/// directories skipped, the same caps, files in sorted order. `None` where
/// ripgrep is not installed, so the walk runs instead.
pub(super) fn search_with_ripgrep(
    request: &SearchRequest,
    cancel: &AtomicBool,
) -> Option<SearchResult> {
    let mut command = Command::new(RIPGREP);
    command
        .args([
            "--line-number",
            "--column",
            "--with-filename",
            "--no-heading",
            "--null",
        ])
        .args(["--color", "never", "--hidden"])
        .args(["--sort", "path", "--no-require-git"])
        // The project's own ignore files, not one above it that would
        // outrank them (see `live_search::rg_args`).
        .arg("--no-ignore-parent")
        .arg(format!("--max-filesize={SEARCH_MAX_FILE_BYTES}"))
        .arg(format!("--max-depth={SEARCH_MAX_DEPTH}"))
        .args(["--fixed-strings", "--case-sensitive", "--word-regexp"]);
    for skipped in SEARCH_SKIP_DIRS {
        command.arg(format!("--glob=!{skipped}"));
    }
    for word in &request.words {
        command.arg("--regexp").arg(word);
    }
    let mut child = command
        .arg(&request.root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut found = SearchResult::default();
    let stdout = child.stdout.take()?;
    for line in BufReader::new(stdout).split(b'\n').map_while(Result::ok) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if let Some(hit) = ripgrep_hit(&line) {
            found.hits.push(hit);
        }
    }
    // Whatever is left unread is not wanted: a cancelled search stops ripgrep
    // rather than letting it walk the rest of the tree.
    let _ = child.kill();
    let _ = child.wait();
    Some(found)
}

/// One line of ripgrep's output, `path NUL line:column:text`, as a hit; its
/// column counts bytes from one.
fn ripgrep_hit(line: &[u8]) -> Option<SearchHit> {
    let split = line.iter().position(|byte| *byte == PATH_END)?;
    let path = PathBuf::from(String::from_utf8_lossy(&line[..split]).to_string());
    let rest = String::from_utf8_lossy(&line[split + 1..]);
    let (number, rest) = rest.split_once(':')?;
    let (column, text) = rest.split_once(':')?;
    Some(SearchHit {
        column: column.parse::<usize>().ok()?.saturating_sub(1),
        line: number.parse().ok()?,
        path,
        text: text.trim_end().chars().take(SEARCH_MAX_LINE).collect(),
    })
}

/// Hand `visit` each file under `dir` a walk reads, `depth` folders down
/// already: links never followed, so a cycle cannot make the walk endless,
/// and the skipped folders never entered. Entries are taken in sorted
/// order, so walking twice reads the same way.
fn walk(
    dir: &Path,
    cancel: &AtomicBool,
    depth: usize,
    visit: &mut dyn FnMut(&Path, &std::fs::Metadata),
) {
    if depth >= SEARCH_MAX_DEPTH || cancel.load(Ordering::Relaxed) {
        return;
    }
    let Ok(reader) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = reader.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if !is_skipped_dir(&path) {
                walk(&path, cancel, depth + 1, visit);
            }
        } else {
            visit(&path, &meta);
        }
    }
}

/// Whether a directory is one walks leave alone.
fn is_skipped_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| SEARCH_SKIP_DIRS.contains(&name))
}

/// Add each line of `path` holding one of `words` whole to `found`. A file
/// that will not read as text is skipped, since searching one prints a
/// screen of control bytes.
fn search_file(path: &Path, words: &[String], found: &mut Vec<SearchHit>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for (index, line) in text.lines().enumerate() {
        if let Some(column) = words.iter().filter_map(|word| word_start(line, word)).min() {
            found.push(SearchHit {
                column,
                line: index + 1,
                path: path.to_path_buf(),
                text: line.trim_end().chars().take(SEARCH_MAX_LINE).collect(),
            });
        }
    }
}

/// Whether `path` names a PDF, by its extension.
fn is_pdf(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

/// Where `line` first holds `word` with no word character on either side of
/// it, the way ripgrep's `--word-regexp` reads a word.
fn word_start(line: &str, word: &str) -> Option<usize> {
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    line.match_indices(word)
        .map(|(start, _)| start)
        .find(|&start| {
            let before = line[..start].chars().next_back();
            let after = line[start + word.len()..].chars().next();
            !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
        })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_word_is_found_only_with_no_word_character_beside_it() {
        assert!(word_start("// TODO: x", "TODO").is_some());
        assert!(word_start("TODO", "TODO").is_some());
        assert!(word_start("fn x() {} // TODO(me)", "TODO").is_some());
        assert!(word_start("TODOS", "TODO").is_none());
        assert!(word_start("MY_TODO", "TODO").is_none());
        assert!(
            word_start("éTODO", "TODO").is_none(),
            "a letter outside ASCII is a word character"
        );
        // The first place it occurs is inside a word; the second is whole.
        assert_eq!(word_start("XTODO TODO", "TODO"), Some(6));
    }

    #[test]
    fn test_the_walk_finds_the_words_as_ripgrep_does() {
        // The walk runs where ripgrep is not installed, and has to answer the
        // same question: whole words, in their own case.
        let dir = std::env::temp_dir().join(format!("winter-words-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("a.rs");
        std::fs::write(&file, "// FIXME later\n// fixme later\n// TODOS\n").expect("a.rs");
        let mut found = Vec::new();
        search_file(
            &file,
            &["TODO".to_string(), "FIXME".to_string()],
            &mut found,
        );
        let lines: Vec<usize> = found.iter().map(|hit| hit.line).collect();
        assert_eq!(lines, [1]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_the_pdfs_under_a_folder_are_listed_and_its_build_output_skipped() {
        let dir = std::env::temp_dir().join(format!("winter-pdfs-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("target")).expect("target");
        std::fs::write(dir.join("paper.PDF"), "x").expect("paper");
        std::fs::write(dir.join("notes.txt"), "x").expect("notes");
        std::fs::write(dir.join("target/built.pdf"), "x").expect("built");
        let pdfs = pdfs_under(&dir, &AtomicBool::new(false));
        assert_eq!(pdfs, [dir.join("paper.PDF")]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
