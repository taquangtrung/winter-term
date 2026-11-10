//! An INI file's outline: its sections (`[Unit]`, `[remote "origin"]`,
//! `[*.md]`), as systemd units, desktop entries, `.editorconfig`, and
//! `setup.cfg` write them. A key under a section is a setting, and is left
//! out.

use winter_model::editor::JumpTarget;

use super::entry;

// ========================================================================
// Constants
// ========================================================================

/// What starts a comment line.
const COMMENTS: [char; 2] = [';', '#'];

// ========================================================================
// Functions
// ========================================================================

/// The outline of the INI file `lines`.
pub(super) fn outline(lines: &[String]) -> Vec<JumpTarget> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(row, line)| {
            let text = line.trim();
            let named = !text.starts_with(COMMENTS) && text.starts_with('[') && text.ends_with(']');
            named.then(|| entry(row, 0, text))
        })
        .collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::tests::{labels, lines};

    #[test]
    fn test_sections_are_listed_and_keys_and_comments_are_not() {
        let doc =
            lines("; [not a section]\n[Unit]\nDescription=x\n\n  [remote \"origin\"]\nurl = [x]\n");
        assert_eq!(
            labels(&outline(&doc)),
            ["    2  [Unit]", "    5  [remote \"origin\"]"]
        );
    }
}
