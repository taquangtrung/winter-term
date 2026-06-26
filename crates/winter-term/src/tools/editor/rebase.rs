//! The plan of an interactive rebase: the `git-rebase-todo` git hands the
//! editor, one commit per line under the action to take on it.
//!
//! The file is text like any other, and the usual keys edit it; what this adds
//! is changing a line's action in one chord, moving a commit up or down, and
//! refusing to finish a plan git would reject.

use std::path::Path;

// ========================================================================
// Constants
// ========================================================================

/// The file git opens an editor on for an interactive rebase's plan.
const TODO_FILE: &str = "git-rebase-todo";

/// What starts a line git left as instructions rather than plan.
const COMMENT_PREFIX: char = '#';

/// The actions a line can take, by the key that sets each, with git's own
/// one-letter abbreviation, which a plan may use in place of the word.
pub(super) const ACTIONS: &[(char, &str)] = &[
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

// ========================================================================
// Functions
// ========================================================================

/// Whether `path` is an interactive rebase's plan.
pub(super) fn is_rebase_todo(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some(TODO_FILE)
}

/// The action set by `key`, if it names one.
pub(super) fn action_of(key: char) -> Option<&'static str> {
    ACTIONS
        .iter()
        .find(|(bound, _)| *bound == key)
        .map(|(_, action)| *action)
}

/// `line` taking `action` instead of the one it has, or `None` for a line
/// that takes no action: a comment, a blank, or something git did not write
/// as a commit.
pub(super) fn with_action(line: &str, action: &str) -> Option<String> {
    let (word, rest) = command_of(line)?;
    let known = ACTIONS
        .iter()
        .any(|(key, name)| *name == word || word.len() == 1 && word.starts_with(*key));
    known.then(|| format!("{action}{rest}"))
}

/// Why git would refuse `lines` as a plan, or `None` when it would take it:
/// the first commit cannot fold into a commit before it, since there is none.
pub(super) fn problem(lines: &[String]) -> Option<String> {
    let (first, _) = lines.iter().find_map(|line| command_of(line))?;
    FOLDING
        .contains(&first)
        .then(|| format!("the first commit cannot {first}: there is nothing before it to fold into"))
}

/// A plan line's action word and everything after it, for a line that is one.
fn command_of(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with(COMMENT_PREFIX) {
        return None;
    }
    let end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    Some((&trimmed[..end], &trimmed[end..]))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_an_action_replaces_the_word_or_abbreviation_and_leaves_comments_alone() {
        assert_eq!(
            with_action("pick abc123 do it", "fixup").as_deref(),
            Some("fixup abc123 do it")
        );
        assert_eq!(with_action("s abc123 do it", "drop").as_deref(), Some("drop abc123 do it"));
        assert_eq!(with_action("# pick is the default", "drop"), None);
        assert_eq!(with_action("", "drop"), None);
        assert_eq!(with_action("exec make", "drop"), None, "not a commit line");
    }

    #[test]
    fn test_a_plan_opening_with_a_fold_is_refused() {
        let plan = |lines: &[&str]| lines.iter().map(|l| l.to_string()).collect::<Vec<_>>();
        assert!(problem(&plan(&["# comment", "squash abc x", "pick def y"])).is_some());
        assert_eq!(problem(&plan(&["pick abc x", "f def y"])), None);
    }
}
