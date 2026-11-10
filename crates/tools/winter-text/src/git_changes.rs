//! What the file in front has changed since its last commit, marked in a
//! gutter lane of its own at the pane's edge, ahead of the line numbers: `+`
//! beside each line added, `~` beside each changed, and `-` where lines were
//! deleted. The file as `HEAD` has it is read with git, and read again every
//! few seconds, so a commit or a checkout made anywhere shows. The lane is
//! there for every file, blank in one git does not track, so the text starts
//! in the same column in all.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use winter_model::editor::{CommandOutput, CommandRequest, EditorOutcome, JobRequest};
use winter_model::paint::{GutterLane, GutterMark, Span, Style};
use winter_vcs::exec;

use super::diff::lines::{self, Compare};
use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// The tag of the read of the file as `HEAD` has it.
pub(super) const TAG_GIT_BASE: &str = "text-git-base";

/// How long the committed text is trusted before it is read again.
const REFRESH: Duration = Duration::from_secs(5);

/// The gutter lane the changes are marked in, ahead of the line numbers.
pub(super) const CHANGE_LANE: GutterLane = GutterLane::Text { width: 1 };

/// The mark beside a line added.
const ADDED: &str = "+";

/// The mark beside a line changed.
const MODIFIED: &str = "~";

/// The mark where lines were deleted: on the line below them, or on the
/// last line where they were at the end.
const DELETED: &str = "-";

/// The id the next read is sent under, so its answer finds its file.
static NEXT_READ: AtomicU64 = AtomicU64::new(1);

// ========================================================================
// Data Structures
// ========================================================================

/// One file's changes against its last commit.
#[derive(Debug, Default)]
pub(super) struct GitChanges {
    /// The id of the read on its way, if one is.
    asked: Option<u64>,
    /// The file as `HEAD` has it, line by line; `None` until git has said,
    /// or where git does not track it.
    base: Option<Vec<String>>,
    /// When the committed text is read again; `None` until the file is first
    /// painted, which asks for it at once.
    due: Option<Instant>,
    /// Each line's change, and the buffer revision it was worked out at.
    marks: RefCell<Option<(usize, Vec<Option<Change>>)>>,
}

/// What became of one line since the last commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Change {
    /// It is new.
    Added,
    /// Lines were deleted just above it.
    DeletedAbove,
    /// It is the last line, and lines were deleted after it.
    DeletedBelow,
    /// It stands where committed lines differed.
    Modified,
}

// ========================================================================
// TextEditor: the file against its last commit
// ========================================================================

impl TextEditor {
    /// Ask git for the file in front as `HEAD` has it, where that is due:
    /// a file on this machine, read, with no read already on its way.
    pub(super) fn git_changes_tick(&mut self, now: Instant) -> Option<EditorOutcome> {
        if !self.git_changes_due(now) {
            return None;
        }
        let path = self.doc().path.clone();
        let (dir, name) = (path.parent()?, path.file_name()?);
        let id = NEXT_READ.fetch_add(1, Ordering::Relaxed);
        let git = &mut self.doc_mut().git;
        git.asked = Some(id);
        git.due = Some(now + REFRESH);
        Some(EditorOutcome::Job(JobRequest::Command(CommandRequest {
            id,
            args: vec![
                "show".to_string(),
                format!("HEAD:./{}", name.to_string_lossy()),
            ],
            cwd: dir.to_path_buf(),
            env: Vec::new(),
            program: "git".to_string(),
            stdin: None,
            tag: TAG_GIT_BASE,
        })))
    }

    /// When the file in front's committed text is next read, for a file it
    /// is read for at all: one on this machine, read and painted, with no
    /// read already on its way.
    pub(super) fn git_changes_next_tick(&self) -> Option<Instant> {
        let doc = self.doc();
        let local = winter_base::remote::parse(&doc.path).is_none();
        match local && !doc.unread && doc.git.asked.is_none() {
            true => doc.git.due,
            false => None,
        }
    }

    /// The file in front is being painted: its committed text is asked for
    /// at once, the first time.
    pub(super) fn git_changes_shown(&mut self) {
        let git = &mut self.doc_mut().git;
        if git.due.is_none() {
            git.due = Some(Instant::now());
        }
    }

    /// Git answered with a file's committed text, or said it has none.
    pub(super) fn git_base_read(&mut self, output: &CommandOutput) -> EditorOutcome {
        let asked = Some(output.id);
        if let Some(doc) = self.docs.iter_mut().find(|doc| doc.git.asked == asked) {
            doc.git.asked = None;
            doc.git.base = exec::succeeded(output).then(|| base_lines(&output.stdout));
            *doc.git.marks.borrow_mut() = None;
        }
        EditorOutcome::Consumed
    }

    /// Line `index`'s mark in the change lane: `+` beside a line added, `~`
    /// beside one changed, `-` where lines were deleted, nothing for one as
    /// committed.
    pub(super) fn change_mark(&self, index: usize) -> Option<GutterMark> {
        let doc = self.doc();
        let base = doc.git.base.as_ref()?;
        let revision = doc.buffer.revision();
        let mut cached = doc.git.marks.borrow_mut();
        if cached.as_ref().map(|(at, _)| *at) != Some(revision) {
            *cached = Some((revision, changes(base, doc.buffer.lines())));
        }
        let change = cached.as_ref()?.1.get(index).copied().flatten()?;
        let (style, text) = match change {
            Change::Added => (Style::GutterAdded, ADDED),
            Change::DeletedAbove | Change::DeletedBelow => (Style::GutterDeleted, DELETED),
            Change::Modified => (Style::GutterModified, MODIFIED),
        };
        Some(GutterMark::Text(Span::new(style, text)))
    }

    /// Line `index`'s gutter marks, in the order of the editor's lanes: what
    /// it changed since the last commit, leftmost (blank where git tracks
    /// nothing of it), then `number`.
    pub(super) fn line_marks(&self, index: usize, number: GutterMark) -> Vec<Option<GutterMark>> {
        vec![self.change_mark(index), Some(number)]
    }

    /// Whether the file in front's committed text is to be read now.
    fn git_changes_due(&self, now: Instant) -> bool {
        self.git_changes_next_tick().is_some_and(|due| due <= now)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The lines of a file as git wrote it out, as the buffer holds them: an
/// empty file still has its one empty line.
fn base_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// What became of each of `current`'s lines since `base`.
fn changes(base: &[String], current: &[String]) -> Vec<Option<Change>> {
    let mut marks = vec![None; current.len()];
    for hunk in lines::diff(base, current, Compare::default()) {
        if hunk.b.is_empty() {
            let at = hunk.b.start;
            let (line, change) = match at < current.len() {
                true => (at, Change::DeletedAbove),
                false => (current.len().saturating_sub(1), Change::DeletedBelow),
            };
            if let Some(mark @ None) = marks.get_mut(line) {
                *mark = Some(change);
            }
            continue;
        }
        let change = match hunk.a.is_empty() {
            true => Change::Added,
            false => Change::Modified,
        };
        for line in hunk.b {
            marks[line] = Some(change);
        }
    }
    marks
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn test_added_changed_and_deleted_lines_are_told_apart() {
        let base = lines(&["a", "b", "c", "d"]);
        let current = lines(&["a", "B", "new", "d"]);
        let marks = changes(&base, &current);
        assert_eq!(marks[0], None, "as committed");
        assert!(
            marks[1..3]
                .iter()
                .all(|mark| *mark == Some(Change::Modified)),
            "{marks:?}"
        );
        assert_eq!(marks[3], None);

        let added = changes(&base, &lines(&["a", "b", "x", "c", "d"]));
        assert_eq!(added[2], Some(Change::Added));
        assert_eq!(added.iter().flatten().count(), 1);

        let deleted = changes(&base, &lines(&["a", "d"]));
        assert_eq!(deleted, [None, Some(Change::DeletedAbove)]);
        let cut = changes(&base, &lines(&["a", "b"]));
        assert_eq!(
            cut,
            [None, Some(Change::DeletedBelow)],
            "deleted after the last"
        );
    }

    #[test]
    fn test_an_empty_committed_file_is_one_empty_line() {
        assert_eq!(base_lines(""), [""]);
        assert_eq!(base_lines("x\r\ny\n"), ["x", "y"]);
    }
}
