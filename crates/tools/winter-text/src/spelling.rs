//! The words of a file's prose that are spelled wrong: the spelling
//! back-end, finding them again as the text changes and publishing them to
//! [`winter_lsp::diagnostics`], which lays them on the text with every other
//! checker's problems. Only the prose is checked: a LaTeX file's text but
//! not its commands, a Markdown file's but not its code. LaTeX, BibTeX, and
//! Markdown files are checked from the moment they open. A long file is
//! checked once typing pauses, off the window's thread.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cherry::DocumentId;
use winter_base::project;
use winter_lsp::diagnostics::{self, Backend};
use winter_lsp::protocol::{self, Diagnostic, Range, Severity};
use winter_model::background::Background;
use winter_spell::{prose_for_extension, shared, Prose};

use super::session::Session;

// ========================================================================
// Constants
// ========================================================================

/// What a card names as having found a word spelled wrong.
const SOURCE: &str = "spelling";

/// How bad a word spelled wrong is: a remark, so a server's own warning or
/// error over it still shows.
const SEVERITY: Severity = Severity::Information;

/// Past this many bytes, a file is checked again only once typing pauses,
/// and off the window's thread.
const LONG_PAST_BYTES: usize = 64 * 1024;

/// How long typing pauses before a long file is checked again.
const SETTLE: Duration = Duration::from_millis(400);

/// The thread a long file is checked on.
const CHECK_THREAD: &str = "winter spelling";

// ========================================================================
// Data Structures
// ========================================================================

/// What one check found: the document's version and the dictionary's
/// generation it checked, and the words spelled wrong.
type Checked = ((u64, u64), Vec<Diagnostic>);

/// Where checking one document stands.
#[derive(Debug)]
pub(crate) struct Spelling {
    /// The document's version and the dictionary's generation it was last
    /// checked at.
    checked: Option<(u64, u64)>,
    /// When it was last checked, for a long one to wait for typing to
    /// pause.
    checked_at: Option<Instant>,
    /// The root of the project the file is in, whose own words count as
    /// spelled right in it.
    project: Option<PathBuf>,
    /// What kind of prose the file is.
    prose: Prose,
    /// A long document's check on its thread, with the version and
    /// generation it checks.
    running: Option<Background<Checked>>,
}

// ========================================================================
// Session: spelling
// ========================================================================

impl Session {
    /// Starts checking `document`, the file at `path`, where it is LaTeX,
    /// BibTeX, or Markdown.
    pub(crate) fn spell_from_the_start(&mut self, document: DocumentId, path: &Path) {
        let Some(prose) = prose_of(path) else {
            return;
        };
        if !matches!(prose, Prose::Latex | Prose::Bibtex | Prose::Markdown) {
            return;
        }
        let spelling = Spelling {
            checked: None,
            checked_at: None,
            project: path.parent().and_then(project::marked_root),
            prose,
            running: None,
        };
        self.spelling.insert(document, spelling);
    }

    /// Checks `document` again where it changed or a word was taken as
    /// spelled right since, publishing what was found: a short one at once,
    /// a long one once typing pauses, on a thread of its own, its findings
    /// taken once it is done.
    pub(crate) fn check_spelling(&mut self, document: DocumentId) {
        let version = self.editor.document_version(document).unwrap_or(0);
        let lines = self.lines(document);
        let Some(path) = self.files.get(&document).map(|file| file.path.clone()) else {
            return;
        };
        let Some(spelling) = self.spelling.get_mut(&document) else {
            return;
        };
        if let Some(running) = spelling.running.take_if(|running| running.is_done()) {
            if let Some((checked, found)) = running.poll() {
                spelling.checked = Some(checked);
                diagnostics::publish(Backend::Spelling, &path, found);
            }
        }
        let now = (version, shared::generation());
        if spelling.checked == Some(now) || spelling.running.is_some() {
            return;
        }
        let long = lines.iter().map(|line| line.len() + 1).sum::<usize>() > LONG_PAST_BYTES;
        let settled = spelling.checked_at.is_none_or(|at| at.elapsed() >= SETTLE);
        if long && !settled {
            return;
        }
        spelling.checked_at = Some(Instant::now());
        let (prose, project) = (spelling.prose, spelling.project.clone());
        if long {
            spelling.running = Some(Background::start(CHECK_THREAD, move |_| {
                (now, find(&lines, prose, project.as_deref()))
            }));
            return;
        }
        spelling.checked = Some(now);
        diagnostics::publish(
            Backend::Spelling,
            &path,
            find(&lines, prose, project.as_deref()),
        );
    }

    /// When `document`'s spelling next wants a look: a long one's check
    /// once typing has paused, or its thread done.
    pub(crate) fn spelling_due(&self, document: DocumentId) -> Option<Instant> {
        let spelling = self.spelling.get(&document)?;
        if spelling.running.is_some() {
            return Some(Instant::now() + SETTLE / 4);
        }
        let changed =
            spelling.checked.map(|(version, _)| version) != self.editor.document_version(document);
        changed.then(|| {
            spelling
                .checked_at
                .map_or_else(Instant::now, |at| at + SETTLE)
        })
    }

    /// Stops checking `document`, its findings taken back.
    pub(crate) fn stop_spelling(&mut self, document: DocumentId) {
        if self.spelling.remove(&document).is_none() {
            return;
        }
        if let Some(file) = self.files.get(&document) {
            diagnostics::publish(Backend::Spelling, &file.path, Vec::new());
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The kind of prose the file at `path` is, by its extension.
fn prose_of(path: &Path) -> Option<Prose> {
    prose_for_extension(path.extension()?.to_str()?)
}

/// The words of `lines`' prose the dictionary does not know, and the
/// project at `project` does not take as spelled right, as the problems
/// the store keeps.
fn find(lines: &[String], prose: Prose, project: Option<&Path>) -> Vec<Diagnostic> {
    let text = lines.join("\n");
    let mut starts = Vec::with_capacity(lines.len());
    let mut at = 0;
    for line in lines {
        starts.push(at);
        at += line.len() + 1;
    }
    shared::dictionary()
        .misspellings_in(&text, prose)
        .into_iter()
        .filter(|token| {
            !text
                .get(token.start..token.end)
                .is_some_and(|word| shared::accepts(word, project))
        })
        .filter_map(|token| {
            let row = starts
                .partition_point(|start| *start <= token.start)
                .saturating_sub(1);
            let (line, start) = (lines.get(row)?, starts.get(row)?);
            let column = |byte: usize| line[..(byte - start).min(line.len())].chars().count();
            let (from, to) = (column(token.start), column(token.end));
            let word: String = line.chars().skip(from).take(to - from).collect();
            Some(Diagnostic {
                code: None,
                message: format!("\"{word}\" is not in the dictionary"),
                range: Range {
                    end: protocol::position_in(lines, row, to),
                    start: protocol::position_in(lines, row, from),
                },
                severity: SEVERITY,
                source: Some(SOURCE.to_owned()),
            })
        })
        .collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_markdown_file_s_misspelling_is_found_on_its_line_and_its_code_is_not() {
        let lines: Vec<String> = ["Hello wrold", "", "`wrold` in code"]
            .map(String::from)
            .to_vec();
        let found = find(&lines, Prose::Markdown, None);
        assert_eq!(found.len(), 1, "{found:?}");
        let range = found[0].range;
        assert_eq!(
            (range.start.line, range.start.character, range.end.character),
            (0, 6, 11)
        );
        assert_eq!(found[0].message, "\"wrold\" is not in the dictionary");
    }
}
