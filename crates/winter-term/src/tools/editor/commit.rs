//! Recalling earlier commit messages into the one being written.
//!
//! `git commit` hands the editor a `COMMIT_EDITMSG` whose message sits above a
//! footer of `#` comment lines. Recall swaps only the part above the footer for
//! a message from the repository's history, `M-p` going further back and
//! `M-n` coming forward again to what was being typed.

use std::path::Path;
use std::process::Command;

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

/// The messages of the repository `message_file` belongs to, or none when git
/// cannot be run there.
fn read_history(message_file: &Path) -> Vec<String> {
    let Some(dir) = message_file.parent() else {
        return Vec::new();
    };
    let format = format!("--format=%B{RECORD_SEPARATOR}");
    let limit = LOG_SCAN_LIMIT.to_string();
    match Command::new("git")
        .args(["log", "-n", &limit, &format])
        .current_dir(dir)
        .output()
    {
        Ok(output) if output.status.success() => {
            distinct_messages(&String::from_utf8_lossy(&output.stdout))
        }
        _ => Vec::new(),
    }
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
        }
    }

    /// Step back to an earlier message. `typed` is the message as it stands,
    /// kept as the draft on the first step. `None` when there is nothing
    /// earlier to show.
    pub(super) fn previous(&mut self, message_file: &Path, typed: String) -> Option<String> {
        let history = self
            .history
            .get_or_insert_with(|| read_history(message_file));
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
        let file = Path::new("COMMIT_EDITMSG");
        assert_eq!(
            recall.previous(file, "typing".into()).as_deref(),
            Some("new")
        );
        assert_eq!(
            recall.previous(file, "ignored".into()).as_deref(),
            Some("old")
        );
        assert_eq!(
            recall.previous(file, "ignored".into()),
            None,
            "oldest reached"
        );
        assert_eq!(recall.next().as_deref(), Some("new"));
        assert_eq!(recall.next().as_deref(), Some("typing"));
        assert_eq!(recall.next(), None, "no longer browsing");
    }

    #[test]
    fn test_only_git_message_files_recall() {
        assert!(is_message_file(Path::new("/r/.git/COMMIT_EDITMSG")));
        assert!(!is_message_file(Path::new("/r/src/main.rs")));
    }
}
