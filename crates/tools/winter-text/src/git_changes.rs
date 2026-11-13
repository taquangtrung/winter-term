//! What a file has changed since its last commit, marked in a gutter lane
//! of its own ahead of the line numbers: `+` beside each line added, `~`
//! beside each changed, and `-` where lines were deleted. The file as
//! `HEAD` has it is read with git, again every few seconds, so a commit or
//! a checkout made anywhere shows. The lane is there for every file, blank
//! in one git does not track, so the text starts in the same column in all.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use cherry::{DecorationKind, DocumentId, GutterLane, Sign};
use winter_model::editor::{CommandOutput, CommandRequest, EditorOutcome, JobRequest};
use winter_model::paint::Style;
use winter_vcs::exec;

use super::diff::lines::{self, Compare};
use super::session::Session;

// ========================================================================
// Constants
// ========================================================================

/// The tag of the read of a file as `HEAD` has it.
pub(super) const TAG_GIT_BASE: &str = "text-git-base";

/// How long the committed text is trusted before it is read again.
const REFRESH: Duration = Duration::from_secs(5);

/// The lane the changes are marked in, and its owner's name.
const LANE: &str = "git";

/// The marks beside a line added, changed, and where lines were deleted.
const ADDED: &str = "+";
const MODIFIED: &str = "~";
const DELETED: &str = "-";

/// The id the next read is sent under, so its answer finds its file.
static NEXT_READ: AtomicU64 = AtomicU64::new(1);

// ========================================================================
// Data Structures
// ========================================================================

/// One file's changes against its last commit.
#[derive(Clone, Debug, Default)]
pub(crate) struct GitBase {
    /// The id of the read on its way, if one is.
    asked: Option<u64>,
    /// The file as `HEAD` has it, line by line; `None` until git has said,
    /// or where git does not track it.
    base: Option<Vec<String>>,
    /// When the committed text is read again; `None` until the file is
    /// first drawn, which asks for it at once.
    due: Option<Instant>,
    /// The document's version the marks were worked out at.
    marked: Option<u64>,
}

/// What became of one line since the last commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Change {
    /// It is new.
    Added,
    /// Lines were deleted just above it, or after it as the last line.
    Deleted,
    /// It stands where committed lines differed.
    Modified,
}

// ========================================================================
// Session: the files against their last commits
// ========================================================================

impl Session {
    /// `document` is drawn: its lane is made, its committed text asked for
    /// the first time, and its marks worked out again where it changed.
    pub(crate) fn git_shown(&mut self, document: DocumentId) {
        let version = self.editor.document_version(document);
        let Some(file) = self.files.get_mut(&document) else {
            return;
        };
        if file.git.due.is_none() {
            file.git.due = Some(Instant::now());
            let lane = GutterLane {
                name: LANE.to_owned(),
                width: 1,
            };
            self.editor.set_gutter_lanes(document, vec![lane]);
        }
        if file.git.marked == version {
            return;
        }
        file.git.marked = version;
        let Some(base) = file.git.base.clone() else {
            return;
        };
        let marks = changes(&base, &self.lines(document))
            .into_iter()
            .enumerate()
            .filter_map(|(line, change)| Some(mark(line, change?)))
            .collect();
        self.editor.set_signs(document, LANE, marks);
    }

    /// When `document`'s committed text is next read.
    pub(crate) fn git_due(&self, document: DocumentId) -> Option<Instant> {
        let git = &self.files.get(&document)?.git;
        git.asked.is_none().then_some(git.due).flatten()
    }

    /// The read of `document`'s committed text, where it is due by `now`.
    pub(crate) fn git_read(&mut self, document: DocumentId, now: Instant) -> Option<EditorOutcome> {
        let file = self.files.get_mut(&document)?;
        if file.git.asked.is_some() || file.git.due.is_none_or(|due| due > now) {
            return None;
        }
        let (dir, name) = (file.path.parent()?, file.path.file_name()?);
        let id = NEXT_READ.fetch_add(1, Ordering::Relaxed);
        file.git.asked = Some(id);
        file.git.due = Some(now + REFRESH);
        Some(EditorOutcome::Job(JobRequest::Command(CommandRequest {
            id,
            args: vec![
                "show".to_owned(),
                format!("HEAD:./{}", name.to_string_lossy()),
            ],
            cwd: dir.to_path_buf(),
            env: Vec::new(),
            program: "git".to_owned(),
            stdin: None,
            tag: TAG_GIT_BASE,
        })))
    }

    /// Git answered `document`'s read with its committed text, or said it
    /// has none: its marks are worked out again at the next draw.
    pub(crate) fn git_answered(&mut self, document: DocumentId, output: &CommandOutput) {
        let Some(file) = self.files.get_mut(&document) else {
            return;
        };
        if file.git.asked != Some(output.id) {
            return;
        }
        file.git.asked = None;
        file.git.base = exec::succeeded(output).then(|| base_lines(&output.stdout));
        file.git.marked = None;
        if file.git.base.is_none() {
            self.editor.set_signs(document, LANE, Vec::new());
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The mark beside `line` for `change`, in the window's color for it.
fn mark(line: usize, change: Change) -> Sign {
    let (style, text) = match change {
        Change::Added => (Style::GutterAdded, ADDED),
        Change::Deleted => (Style::GutterDeleted, DELETED),
        Change::Modified => (Style::GutterModified, MODIFIED),
    };
    let kind = DecorationKind::Background(winter_syntax::highlight_id(style));
    Sign::new(line, text, kind).in_lane(LANE)
}

/// The lines of a file as git wrote it out, as the document holds them:
/// an empty file still has its one empty line.
fn base_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
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
            let line = hunk.b.start.min(current.len().saturating_sub(1));
            if let Some(mark @ None) = marks.get_mut(line) {
                *mark = Some(Change::Deleted);
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
        let marks = changes(&base, &lines(&["a", "B", "new", "d"]));
        assert_eq!(
            marks,
            [None, Some(Change::Modified), Some(Change::Modified), None]
        );
        let added = changes(&base, &lines(&["a", "b", "x", "c", "d"]));
        assert_eq!(added[2], Some(Change::Added));
        assert_eq!(added.iter().flatten().count(), 1);
        assert_eq!(
            changes(&base, &lines(&["a", "d"])),
            [None, Some(Change::Deleted)]
        );
        assert_eq!(
            changes(&base, &lines(&["a", "b"])),
            [None, Some(Change::Deleted)],
            "deleted after the last line"
        );
    }

    #[test]
    fn test_git_s_answer_marks_the_lines_changed_since_at_the_next_draw() {
        let dir = std::env::temp_dir().join(format!("winter-git-marks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a folder");
        let path = dir.join("notes.txt");
        std::fs::write(&path, "one\nTWO\nthree\n").expect("written");
        crate::session::with(|session| {
            let (document, _) = session.open_view(&path).expect("opens");
            session.git_shown(document);
            let Some(EditorOutcome::Job(JobRequest::Command(read))) =
                session.git_read(document, Instant::now())
            else {
                panic!("the committed text is asked for at once");
            };
            let output = CommandOutput {
                code: Some(0),
                id: read.id,
                stderr: String::new(),
                stdout: "one\ntwo\nthree\n".to_owned(),
                tag: TAG_GIT_BASE,
            };
            session.git_answered(document, &output);
            session.git_shown(document);
            let signs = session.editor.signs(document, LANE);
            assert_eq!(signs, [mark(1, Change::Modified)], "TWO changed");
        });
    }

    #[test]
    fn test_an_empty_committed_file_is_one_empty_line() {
        assert_eq!(base_lines(""), [""]);
        assert_eq!(base_lines("x\r\ny\n"), ["x", "y"]);
    }
}
