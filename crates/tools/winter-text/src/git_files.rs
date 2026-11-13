//! The files git hands the editor: a message (`COMMIT_EDITMSG`,
//! `MERGE_MSG`, `TAG_EDITMSG`) and an interactive rebase's plan
//! (`git-rebase-todo`). They are text like any other, and Cherry's keys
//! edit them; what this adds are keys of their own, bound in their
//! documents alone as Cherry's host commands:
//!
//! - a message is finished (`C-c C-c`) or cancelled (`C-c C-k`, or `:q!`,
//!   which empties it so git stops at an empty message), swapped for an
//!   earlier message of the repository's (`C-M-p`, `C-M-n` coming forward
//!   again to what was typed), and its summary marked past the width it
//!   reads well at;
//! - a plan's lines take an action in one key (`p`, `r`, `e`, `s`, `f`, `d`
//!   in Normal mode, `C-c` and the letter anywhere, over a selection's
//!   lines too), move up or down (`C-c k`, `C-c j`), and are carried out
//!   (`C-c C-c`) unless git would refuse them, or aborted (`C-c C-k`).

use std::path::Path;

use cherry::{Decoration, DecorationKind, DocumentId, Editor, Mode};
use winter_model::editor::{EditorOutcome, JobRequest, Notice};
use winter_vcs::exec;

use super::language::{MESSAGE_FILES, REBASE_TODO};
use super::session::{self, Session};
use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// What starts a line git left as instructions rather than message or plan.
const COMMENT_PREFIX: char = '#';

/// How long a summary line reads well, past which the rest is marked: what
/// `git log --oneline` and the forges show of it before cutting it off.
const SUMMARY_WIDTH: usize = 72;

/// The owner of the mark on a summary past its width.
const SUMMARY: &str = "summary";

/// How many distinct earlier messages can be recalled.
const HISTORY_LIMIT: usize = 10;

/// How many commits are read to find them, generous so a run of repeated
/// messages ("wip", "wip") does not starve the search.
const LOG_SCAN_LIMIT: usize = 100;

/// Ends each message in `git log`'s output. A record separator rather than
/// a blank line, since a multi-paragraph message has blank lines of its own.
const RECORD_SEPARATOR: char = '\u{1e}';

/// The actions a plan line can take, each with git's own one-letter
/// abbreviation, which a plan may use in place of the word.
const ACTIONS: [(char, &str); 6] = [
    ('p', "pick"),
    ('r', "reword"),
    ('e', "edit"),
    ('s', "squash"),
    ('f', "fixup"),
    ('d', "drop"),
];

/// The actions that fold a commit into the one before it, which the first
/// commit of a plan has none of.
const FOLDING: [&str; 4] = ["squash", "s", "fixup", "f"];

/// The host commands of a message.
const MESSAGE_FINISH: &str = "git-message-finish";
const MESSAGE_CANCEL: &str = "git-message-cancel";
const MESSAGE_EARLIER: &str = "git-message-earlier";
const MESSAGE_LATER: &str = "git-message-later";

/// The host commands of a plan, besides one setting each action, named
/// after [`REBASE_SET`] and the action.
const REBASE_FINISH: &str = "git-rebase-finish";
const REBASE_ABORT: &str = "git-rebase-abort";
const REBASE_DOWN: &str = "git-rebase-move-down";
const REBASE_UP: &str = "git-rebase-move-up";
const REBASE_SET: &str = "git-rebase-";

/// The keys of a message's commands and of a plan's, in every mode they
/// are typed in.
const MESSAGE_KEYS: [(&str, &[&str]); 4] = [
    (MESSAGE_FINISH, &["C-c C-c", "C-S-c"]),
    (MESSAGE_CANCEL, &["C-c C-k"]),
    (MESSAGE_EARLIER, &["C-M-p"]),
    (MESSAGE_LATER, &["C-M-n"]),
];
const REBASE_KEYS: [(&str, &[&str]); 4] = [
    (REBASE_FINISH, &["C-c C-c", "C-S-c"]),
    (REBASE_ABORT, &["C-c C-k"]),
    (REBASE_DOWN, &["C-c j"]),
    (REBASE_UP, &["C-c k"]),
];

/// The modes a git file's chords are typed in.
const CHORD_MODES: [Mode; 3] = [Mode::Insert, Mode::Normal, Mode::Visual];

/// What is said of an action set on a line that holds no commit.
const NOT_A_COMMIT: &str = "not a commit line";

// ========================================================================
// Data Structures
// ========================================================================

/// Where recalling earlier messages has got to in one tab.
#[derive(Debug, Default)]
pub(super) struct MessageRecall {
    /// What was typed before the first step back, restored by stepping
    /// forward past the newest message.
    draft: String,
    /// Earlier messages, oldest first; read once, on the first step back.
    history: Option<Vec<String>>,
    /// The message showing, or `None` while the typed text is.
    shown: Option<usize>,
    /// Whether a step back is waiting on the history being read, to be
    /// taken once it has been.
    waiting: bool,
}

/// What a git file is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GitFile {
    /// A message: a commit's, a merge's, or a tag's.
    Message,
    /// An interactive rebase's plan.
    Plan,
}

// ========================================================================
// MessageRecall Implementations
// ========================================================================

impl MessageRecall {
    /// The read of the history a first step back needs, when it has not
    /// been read yet: the messages of the repository `message_file` is in.
    /// The step is taken once [`Self::take_history`] has the answer, so the
    /// window never waits on git for it.
    fn history_request(&mut self, message_file: &Path) -> Option<JobRequest> {
        if self.history.is_some() || self.waiting {
            return None;
        }
        let dir = message_file.parent()?;
        self.waiting = true;
        Some(exec::messages(dir, LOG_SCAN_LIMIT, RECORD_SEPARATOR))
    }

    /// Takes the history git answered with, or none when it could not be
    /// read. Whether a step back was waiting on it.
    fn take_history(&mut self, log: Option<&str>) -> bool {
        self.history = Some(log.map(distinct_messages).unwrap_or_default());
        std::mem::take(&mut self.waiting)
    }

    /// Steps back to an earlier message, `typed` kept as the draft on the
    /// first step. `None` with nothing earlier to show, or no history read.
    fn previous(&mut self, typed: String) -> Option<String> {
        let history = self.history.as_ref()?;
        let at = match self.shown {
            None => history.len().checked_sub(1)?,
            Some(shown) => shown.checked_sub(1)?,
        };
        if self.shown.is_none() {
            self.draft = typed;
        }
        self.shown = Some(at);
        history.get(at).cloned()
    }

    /// Steps forward, back to the draft after the newest message. `None`
    /// when no recall is under way.
    fn next(&mut self) -> Option<String> {
        let shown = self.shown?;
        let history = self.history.as_ref()?;
        match history.get(shown + 1) {
            Some(message) => {
                self.shown = Some(shown + 1);
                Some(message.clone())
            }
            None => {
                self.shown = None;
                Some(std::mem::take(&mut self.draft))
            }
        }
    }
}

// ========================================================================
// Session: git's files
// ========================================================================

impl Session {
    /// Binds the keys of the git file at `path`, open as `document`, in it
    /// alone.
    pub(crate) fn bind_git_keys(&mut self, document: DocumentId, path: &Path) {
        let (chords, letters): (&[(&str, &[&str])], bool) = match git_file(path) {
            Some(GitFile::Message) => (&MESSAGE_KEYS, false),
            Some(GitFile::Plan) => (&REBASE_KEYS, true),
            None => return,
        };
        for (id, keys) in chords {
            for mode in CHORD_MODES {
                bind(&mut self.editor, document, mode, keys, id);
            }
        }
        if !letters {
            return;
        }
        for (letter, action) in ACTIONS {
            let id = format!("{REBASE_SET}{action}");
            let chord = format!("C-c {letter}");
            for mode in CHORD_MODES {
                bind(&mut self.editor, document, mode, &[&chord], &id);
            }
            let letter = letter.to_string();
            bind(&mut self.editor, document, Mode::Normal, &[&letter], &id);
        }
    }

    /// Marks the summary of `document`, a message, past the width it reads
    /// well at, as its text now reads.
    pub(crate) fn mark_summary(&mut self, document: DocumentId) {
        let version = self.editor.document_version(document);
        let Some(file) = self.files.get_mut(&document) else {
            return;
        };
        if git_file(&file.path) != Some(GitFile::Message) || file.summary_marked == version {
            return;
        }
        file.summary_marked = version;
        let summary = self
            .editor
            .document(document)
            .and_then(|text| text.line_text(0))
            .unwrap_or_default();
        let length = summary.chars().count();
        let marks = match length > SUMMARY_WIDTH {
            true => vec![Decoration::new(
                SUMMARY_WIDTH..length,
                DecorationKind::Warning,
            )],
            false => Vec::new(),
        };
        self.editor.set_decorations(document, SUMMARY, marks);
    }
}

// ========================================================================
// TextEditor: git's files
// ========================================================================

impl TextEditor {
    /// Whether the tab holds a file git hands the editor, which git
    /// reopens itself rather than a restored session.
    pub(super) fn holds_git_file(&self) -> bool {
        git_file(&self.path).is_some()
    }

    /// Whether the tab holds a message, which opens ready to type.
    pub(super) fn holds_message(&self) -> bool {
        git_file(&self.path) == Some(GitFile::Message)
    }

    /// Runs the git file command `id`: what the window is to do, or `None`
    /// for a command of another's.
    pub(super) fn run_git_command(&mut self, id: &str) -> Option<EditorOutcome> {
        Some(match id {
            MESSAGE_FINISH => self.write_and_close(),
            MESSAGE_CANCEL | REBASE_ABORT => self.empty_and_close(),
            MESSAGE_EARLIER => self.recall_message(true),
            MESSAGE_LATER => self.recall_message(false),
            REBASE_FINISH => match plan_problem(&self.lines()) {
                Some(problem) => {
                    self.notice = Some(Notice::error(problem));
                    EditorOutcome::Consumed
                }
                None => self.write_and_close(),
            },
            REBASE_DOWN => self.move_plan_line(true),
            REBASE_UP => self.move_plan_line(false),
            id => {
                let action = id.strip_prefix(REBASE_SET)?;
                ACTIONS.iter().find(|(_, known)| *known == action)?;
                self.set_plan_action(action)
            }
        })
    }

    /// `:q!` in a message: cancelled as git understands, by emptying it.
    /// `None` for any other file.
    pub(super) fn cancel_message(&mut self) -> Option<EditorOutcome> {
        self.holds_message().then(|| self.empty_and_close())
    }

    /// Takes the messages git answered a recall's read with, where `tag`
    /// is that read's: the step back waiting on them taken.
    pub(super) fn take_messages(&mut self, tag: &str, log: Option<&str>) -> Option<EditorOutcome> {
        if tag != exec::TAG_MESSAGES {
            return None;
        }
        Some(match self.recall.take_history(log) {
            true => self.recall_message(true),
            false => EditorOutcome::Consumed,
        })
    }

    /// Writes the file and closes the tab, which git takes as done.
    fn write_and_close(&mut self) -> EditorOutcome {
        match self.write() {
            EditorOutcome::Wrote(_) => EditorOutcome::Close,
            refused => refused,
        }
    }

    /// Empties the file, writes it, and closes the tab: how git is told to
    /// stop, at an empty message or an empty plan.
    fn empty_and_close(&mut self) -> EditorOutcome {
        self.replace_lines(0, usize::MAX, "");
        self.write_and_close()
    }

    /// Swaps the message above git's comments for an earlier one (`back`)
    /// or the one after it. The first step back asks git for the history,
    /// and is taken when it comes back.
    fn recall_message(&mut self, back: bool) -> EditorOutcome {
        if back {
            if let Some(request) = self.recall.history_request(&self.path) {
                return EditorOutcome::Job(request);
            }
        }
        let lines = self.lines();
        let rows = message_rows(&lines);
        let recalled = match back {
            true => self.recall.previous(lines[..rows].join("\n")),
            false => self.recall.next(),
        };
        if let Some(message) = recalled {
            self.replace_lines(0, rows, &message);
        }
        EditorOutcome::Consumed
    }

    /// Sets `action` on the plan lines the selection covers, or on the
    /// cursor's line, and steps on past them in Normal mode. Lines that
    /// are no commit stay as they are.
    fn set_plan_action(&mut self, action: &str) -> EditorOutcome {
        let lines = self.lines();
        let (first, last) = self.selected_lines();
        let Some(chosen) = lines.get(first..=last) else {
            return EditorOutcome::Consumed;
        };
        let changed: Vec<Option<String>> = chosen
            .iter()
            .map(|line| with_action(line, action))
            .collect();
        if changed.iter().all(Option::is_none) {
            self.notice = Some(Notice::error(NOT_A_COMMIT));
            return EditorOutcome::Consumed;
        }
        let text = chosen
            .iter()
            .zip(changed)
            .map(|(line, changed)| changed.unwrap_or_else(|| line.clone()))
            .collect::<Vec<_>>()
            .join("\n");
        self.replace_lines(first, last + 1 - first, &text);
        let view = self.view_id();
        session::with(|session| {
            if session
                .editor
                .view(view)
                .is_some_and(|view| view.visual_anchor().is_some())
            {
                let _ = session.editor.run_command(view, "exit-visual", None);
            }
        });
        self.move_to(last + 1, 0);
        EditorOutcome::Consumed
    }

    /// Swaps the cursor's plan line with the one below it (`down`) or above
    /// it, the cursor going with it.
    fn move_plan_line(&mut self, down: bool) -> EditorOutcome {
        let lines = self.lines();
        let (row, column) = self.cursor_line_column();
        let other = match down {
            true => row + 1,
            false => row.wrapping_sub(1),
        };
        if other >= lines.len() {
            return EditorOutcome::Consumed;
        }
        let first = row.min(other);
        let swapped = format!("{}\n{}", lines[first + 1], lines[first]);
        self.replace_lines(first, 2, &swapped);
        self.move_to(other, column);
        EditorOutcome::Consumed
    }

    /// The first and last lines the selection covers, or the cursor's line
    /// twice.
    fn selected_lines(&self) -> (usize, usize) {
        let view = self.view_id();
        session::with(|session| {
            let line = |offset| {
                session
                    .editor
                    .document(self.document)
                    .and_then(|text| text.char_to_position(offset))
                    .map_or(0, |at| at.line)
            };
            let Some(shown) = session.editor.view(view) else {
                return (0, 0);
            };
            let cursor = line(shown.cursor());
            let anchor = shown.visual_anchor().map_or(cursor, line);
            (anchor.min(cursor), anchor.max(cursor))
        })
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What the file at `path` is to git, by its name.
fn git_file(path: &Path) -> Option<GitFile> {
    let name = path.file_name()?.to_str()?;
    match name {
        REBASE_TODO => Some(GitFile::Plan),
        name if MESSAGE_FILES.contains(&name) => Some(GitFile::Message),
        _ => None,
    }
}

/// Binds `keys` to the host command `id` in `mode` in `document` alone.
fn bind(editor: &mut Editor, document: DocumentId, mode: Mode, keys: &[&str], id: &str) {
    let bound = editor.bind_in_document(document, mode, keys, id);
    debug_assert!(bound.is_ok(), "{id} takes {keys:?}: {bound:?}");
}

/// Every git file command's id, registered once with the editor.
pub(crate) fn host_commands() -> Vec<String> {
    let chords = MESSAGE_KEYS
        .iter()
        .chain(&REBASE_KEYS)
        .map(|(id, _)| id.to_string());
    let actions = ACTIONS
        .iter()
        .map(|(_, action)| format!("{REBASE_SET}{action}"));
    chords.chain(actions).collect()
}

/// Whether `line` is one git left as instructions.
fn is_comment_line(line: &str) -> bool {
    line.starts_with(COMMENT_PREFIX)
}

/// How many lines at the top are the message: those above the first
/// comment.
fn message_rows(lines: &[String]) -> usize {
    lines
        .iter()
        .position(|line| is_comment_line(line))
        .unwrap_or(lines.len())
}

/// The newest distinct messages of `log`'s output, oldest first, each
/// without its comment lines.
fn distinct_messages(log: &str) -> Vec<String> {
    let mut messages: Vec<String> = Vec::new();
    for message in log.split(RECORD_SEPARATOR) {
        if messages.len() >= HISTORY_LIMIT {
            break;
        }
        let message = message
            .lines()
            .filter(|line| !is_comment_line(line))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();
        if !message.is_empty() && !messages.contains(&message) {
            messages.push(message);
        }
    }
    messages.reverse();
    messages
}

/// A plan line's action word and everything after it, for a line that is
/// one.
fn command_of(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with(COMMENT_PREFIX) {
        return None;
    }
    let end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    Some((&trimmed[..end], &trimmed[end..]))
}

/// `line` taking `action` instead of the one it has, or `None` for a line
/// that takes no action: a comment, a blank, or something git did not
/// write as a commit.
fn with_action(line: &str, action: &str) -> Option<String> {
    let (word, rest) = command_of(line)?;
    let known = ACTIONS
        .iter()
        .any(|(key, name)| *name == word || word.len() == 1 && word.starts_with(*key));
    known.then(|| format!("{action}{rest}"))
}

/// Why git would refuse `lines` as a plan, or `None` when it would take
/// it: the first commit cannot fold into a commit before it, since there
/// is none.
fn plan_problem(lines: &[String]) -> Option<String> {
    let (first, _) = lines.iter().find_map(|line| command_of(line))?;
    FOLDING.contains(&first).then(|| {
        format!("the first commit cannot {first}: there is nothing before it to fold into")
    })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use winter_model::editor::Page;
    use winter_model::input::{Key, KeyCode};

    use super::*;

    /// `code` pressed, with Ctrl and Alt as asked.
    fn press(code: KeyCode, ctrl: bool, alt: bool) -> Key {
        Key {
            alt,
            code,
            ctrl,
            shift: false,
            sup: false,
        }
    }

    /// The keys of `typed`, each a plain character.
    fn letters(editor: &mut TextEditor, typed: &str) -> Vec<EditorOutcome> {
        typed
            .chars()
            .map(|c| editor.on_key(&press(KeyCode::Char(c), false, false)))
            .collect()
    }

    /// `C-c` then `c`, with Ctrl where `ctrl`.
    fn chord(editor: &mut TextEditor, c: char, ctrl: bool) -> EditorOutcome {
        editor.on_key(&press(KeyCode::Char('c'), true, false));
        editor.on_key(&press(KeyCode::Char(c), ctrl, false))
    }

    /// A file named `name` holding `text`, in a folder of its own named for
    /// `tag`.
    fn git_file_at(tag: &str, name: &str, text: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("winter-git-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a folder");
        let path = dir.join(name);
        std::fs::write(&path, text).expect("written");
        path
    }

    #[test]
    fn test_history_is_distinct_newest_kept_and_oldest_first() {
        let log = "fix\u{1e}wip\u{1e}\nwip\u{1e}feat\n\nbody\n# a comment\u{1e}";
        assert_eq!(distinct_messages(log), ["feat\n\nbody", "wip", "fix"]);
        let long: String = (0..30).map(|n| format!("m{n}\u{1e}")).collect();
        assert_eq!(distinct_messages(&long).len(), HISTORY_LIMIT);
    }

    #[test]
    fn test_a_plan_line_takes_an_action_and_a_first_fold_is_refused() {
        assert_eq!(
            with_action("p abc one", "fixup").as_deref(),
            Some("fixup abc one")
        );
        assert_eq!(with_action("# pick abc", "drop"), None, "a comment");
        assert_eq!(with_action("exec make", "drop"), None, "no commit");
        let plan = |lines: &[&str]| {
            lines
                .iter()
                .map(|line| line.to_string())
                .collect::<Vec<_>>()
        };
        assert!(plan_problem(&plan(&["# note", "s abc one", "pick def two"])).is_some());
        assert!(plan_problem(&plan(&["pick abc one", "squash def two"])).is_none());
    }

    #[test]
    fn test_a_message_opens_typing_and_recalls_earlier_ones_back_to_the_draft() {
        let path = git_file_at("recall", MESSAGE_FILES[0], "\n# Please enter\n");
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        letters(&mut editor, "draft");
        assert_eq!(editor.lines()[0], "draft", "typed straight in");
        editor.recall.history = Some(vec!["old".to_string(), "new\n\nbody".to_string()]);
        let back = press(KeyCode::Char('p'), true, true);
        editor.on_key(&back);
        assert_eq!(editor.lines(), ["new", "", "body", "# Please enter"]);
        editor.on_key(&back);
        assert_eq!(editor.lines()[..2], ["old", "# Please enter"]);
        editor.on_key(&press(KeyCode::Char('n'), true, true));
        editor.on_key(&press(KeyCode::Char('n'), true, true));
        assert_eq!(
            editor.lines()[..2],
            ["draft", "# Please enter"],
            "the draft back"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_a_cancelled_message_is_emptied_written_and_closed() {
        let path = git_file_at(
            "cancel",
            MESSAGE_FILES[0],
            "half a thought\n# Please enter\n",
        );
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        assert_eq!(chord(&mut editor, 'k', true), EditorOutcome::Close);
        assert_eq!(std::fs::read_to_string(&path).expect("read").trim(), "");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_a_summary_past_its_width_is_marked() {
        let summary = "x".repeat(SUMMARY_WIDTH + 8);
        let path = git_file_at("summary", MESSAGE_FILES[0], &format!("{summary}\n"));
        let editor = TextEditor::new(path.clone(), None).expect("opens");
        session::with(|session| {
            session.mark_summary(editor.document);
            let marks = session.editor.decorations(editor.document, SUMMARY);
            assert_eq!(marks.len(), 1);
            assert_eq!(marks[0].range, SUMMARY_WIDTH..SUMMARY_WIDTH + 8);
        });
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_a_plan_is_reordered_by_letters_and_carried_out_only_when_git_takes_it() {
        let text = "pick aaa one\npick bbb two\n# Commands:\n";
        let path = git_file_at("plan", REBASE_TODO, text);
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        letters(&mut editor, "r");
        assert_eq!(editor.lines()[..2], ["reword aaa one", "pick bbb two"]);
        assert_eq!(editor.cursor_line_column().0, 1, "stepped on");
        chord(&mut editor, 'k', false);
        assert_eq!(editor.lines()[..2], ["pick bbb two", "reword aaa one"]);
        assert_eq!(editor.cursor_line_column().0, 0, "the line carried up");
        letters(&mut editor, "s");
        assert_eq!(
            chord(&mut editor, 'c', true),
            EditorOutcome::Consumed,
            "refused"
        );
        assert!(editor.take_notice().is_some(), "and why said");
        letters(&mut editor, "kf");
        assert_eq!(editor.lines()[0], "fixup bbb two");
        letters(&mut editor, "kp");
        assert_eq!(chord(&mut editor, 'c', true), EditorOutcome::Close);
        let written = std::fs::read_to_string(&path).expect("read");
        assert!(
            written.starts_with("pick bbb two\nreword aaa one\n"),
            "{written}"
        );
        let _ = std::fs::remove_file(path);
    }
}
