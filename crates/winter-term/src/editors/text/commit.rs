//! Recalling earlier commit messages into the one being written.
//!
//! `git commit` hands the editor a `COMMIT_EDITMSG` whose message sits above a
//! footer of `#` comment lines. Recall swaps only the part above the footer for
//! a message from the repository's history, `M-p` going further back and
//! `M-n` coming forward again to what was being typed.

use std::path::Path;

use crate::editors::git::exec;
use crate::model::editor::JobRequest;

// ========================================================================
// Constants
// ========================================================================

/// The file git opens an editor on for a commit's message.
const COMMIT_FILE: &str = "COMMIT_EDITMSG";

/// Files git opens an editor on for a message, which is where recall applies.
const MESSAGE_FILES: [&str; 3] = [COMMIT_FILE, "MERGE_MSG", "TAG_EDITMSG"];

/// How many distinct earlier messages can be recalled.
const HISTORY_LIMIT: usize = 10;

/// How many commits are read to find them, generous so a run of repeated
/// messages ("wip", "wip") does not starve the search.
const LOG_SCAN_LIMIT: usize = 100;

/// Ends each message in `git log`'s output. A record separator rather than a
/// blank line, since a multi-paragraph message has blank lines of its own.
const RECORD_SEPARATOR: char = '\u{1e}';

/// What starts a line git left as instructions rather than message.
const COMMENT_PREFIX: char = '#';

/// How long a summary line reads well, past which the rest is marked: what
/// `git log --oneline` and the forges show of it before cutting it off.
pub(super) const SUMMARY_WIDTH: usize = 72;

// ========================================================================
// Data Structures
// ========================================================================

/// Where recall has got to in one editing session.
#[derive(Debug, Default)]
pub(super) struct MessageRecall {
    /// What was typed before the first step back, restored by stepping forward
    /// past the newest message.
    draft: String,
    /// Earlier messages, oldest first; read once, on the first step back.
    history: Option<Vec<String>>,
    /// Whether a step back is waiting on the history being read, to be taken
    /// once it has been.
    waiting: bool,
    /// The message showing, or `None` while the typed text is.
    shown: Option<usize>,
}

// ========================================================================
// Free functions
// ========================================================================

/// Whether `path` is a file git edits a message in.
pub(super) fn is_message_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| MESSAGE_FILES.contains(&name))
}

/// Whether `path` is the message of a commit being made, as opposed to a merge
/// or a tag.
pub(super) fn is_commit_message(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some(COMMIT_FILE)
}

/// Whether `line` is one git left as instructions, which it drops from the
/// message.
pub(super) fn is_comment_line(line: &str) -> bool {
    line.starts_with(COMMENT_PREFIX)
}

/// How many rows at the top are the message: those above the first comment.
pub(super) fn message_rows(lines: &[String]) -> usize {
    lines
        .iter()
        .position(|line| is_comment_line(line))
        .unwrap_or(lines.len())
}

/// `text` without its comment lines, trimmed.
fn strip_comments(text: &str) -> String {
    text.lines()
        .filter(|line| !is_comment_line(line))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// The newest distinct messages of `log`'s output, oldest first.
fn distinct_messages(log: &str) -> Vec<String> {
    let mut messages: Vec<String> = Vec::new();
    for message in log.split(RECORD_SEPARATOR).map(strip_comments) {
        if messages.len() >= HISTORY_LIMIT {
            break;
        }
        if !message.is_empty() && !messages.contains(&message) {
            messages.push(message);
        }
    }
    messages.reverse();
    messages
}

// ========================================================================
// MessageRecall
// ========================================================================

impl MessageRecall {
    /// Recall over `history`, oldest first, without asking git.
    #[cfg(test)]
    pub(super) fn with_history(history: &[&str]) -> Self {
        Self {
            draft: String::new(),
            history: Some(history.iter().map(|m| m.to_string()).collect()),
            shown: None,
            waiting: false,
        }
    }

    /// The read of the history a first step back needs, when it has not been
    /// read yet: the messages of the repository `message_file` belongs to.
    /// The step is taken once [`MessageRecall::take_history`] has the answer,
    /// so the window never waits on git for it.
    pub(super) fn history_request(&mut self, message_file: &Path) -> Option<JobRequest> {
        if self.history.is_some() || self.waiting {
            return None;
        }
        let dir = message_file.parent()?;
        self.waiting = true;
        Some(exec::messages(dir, LOG_SCAN_LIMIT, RECORD_SEPARATOR))
    }

    /// Take the history git answered with, or none when it could not be read,
    /// and whether a step back was waiting on it.
    pub(super) fn take_history(&mut self, log: Option<&str>) -> bool {
        self.history = Some(log.map(distinct_messages).unwrap_or_default());
        std::mem::take(&mut self.waiting)
    }

    /// Step back to an earlier message. `typed` is the message as it stands,
    /// kept as the draft on the first step. `None` when there is nothing
    /// earlier to show, or no history read yet.
    pub(super) fn previous(&mut self, typed: String) -> Option<String> {
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

    /// Step forward, back to the draft after the newest message. `None` when
    /// recall is not under way.
    pub(super) fn next(&mut self) -> Option<String> {
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
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn recall_over(history: &[&str]) -> MessageRecall {
        MessageRecall::with_history(history)
    }

    #[test]
    fn test_message_ends_at_the_first_comment_line() {
        let lines: Vec<String> = ["", "# Please enter", "# more"]
            .iter()
            .map(|l| l.to_string())
            .collect();
        assert_eq!(message_rows(&lines), 1);
        assert_eq!(message_rows(&lines[..1]), 1, "no footer: all of it");
    }

    #[test]
    fn test_history_is_distinct_newest_kept_and_oldest_first() {
        let log = "fix\u{1e}wip\u{1e}\nwip\u{1e}feat\n\nbody\u{1e}";
        assert_eq!(
            distinct_messages(log),
            vec!["feat\n\nbody", "wip", "fix"],
            "the repeated wip collapses; the log's newest comes last"
        );
    }

    #[test]
    fn test_history_stops_at_the_limit() {
        let log: String = (0..30).map(|n| format!("m{n}\u{1e}")).collect();
        assert_eq!(distinct_messages(&log).len(), HISTORY_LIMIT);
    }

    #[test]
    fn test_stepping_back_then_forward_returns_to_the_draft() {
        let mut recall = recall_over(&["old", "new"]);
        assert_eq!(recall.previous("typing".into()).as_deref(), Some("new"));
        assert_eq!(recall.previous("ignored".into()).as_deref(), Some("old"));
        assert_eq!(recall.previous("ignored".into()), None, "oldest reached");
        assert_eq!(recall.next().as_deref(), Some("new"));
        assert_eq!(recall.next().as_deref(), Some("typing"));
        assert_eq!(recall.next(), None, "no longer browsing");
    }

    #[test]
    fn test_the_history_is_asked_for_once_and_the_waiting_step_taken_after() {
        let mut recall = MessageRecall::default();
        let file = Path::new("/r/.git/COMMIT_EDITMSG");
        assert_eq!(recall.previous("typing".into()), None, "nothing read yet");
        let request = recall.history_request(file).expect("asks git");
        let JobRequest::Command(command) = request else {
            panic!("a command");
        };
        assert_eq!(command.cwd, Path::new("/r/.git"));
        assert!(recall.history_request(file).is_none(), "asked once");
        assert!(recall.take_history(Some("fix\u{1e}")), "a step was waiting");
        assert_eq!(recall.previous("typing".into()).as_deref(), Some("fix"));
        assert!(recall.history_request(file).is_none(), "already read");
    }

    #[test]
    fn test_a_history_that_could_not_be_read_recalls_nothing() {
        let mut recall = MessageRecall::default();
        recall.history_request(Path::new("/r/.git/COMMIT_EDITMSG"));
        recall.take_history(None);
        assert_eq!(recall.previous("typing".into()), None);
    }

    #[test]
    fn test_only_git_message_files_recall() {
        assert!(is_message_file(Path::new("/r/.git/COMMIT_EDITMSG")));
        assert!(!is_message_file(Path::new("/r/src/main.rs")));
    }
}
