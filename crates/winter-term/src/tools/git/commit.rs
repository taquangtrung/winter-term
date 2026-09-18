//! One commit's content, parsed into the shape the commit view draws.
//!
//! `git show` writes a summary followed by one `diff --git` section per file.
//! The view needs those files addressable rather than as a wall of lines: a file
//! carries an icon and folds its hunks away, and a hunk folds to its own header,
//! which a flat line list cannot express. Parsing is pure so it can be tested
//! without running git.

use super::diff::{parse_diff, Hunk};

// ========================================================================
// Constants
// ========================================================================

/// The line that opens one file's section of a `git show`.
const FILE_MARK: &str = "diff --git ";

/// Lines in a file's section that say the file was added.
const NEW_FILE_MARK: &str = "new file mode ";

/// Lines in a file's section that say the file was deleted.
const DELETED_FILE_MARK: &str = "deleted file mode ";

/// Lines in a file's section that say the file was renamed.
const RENAME_MARKS: [&str; 2] = ["rename from ", "rename to "];

/// Lines of file-header detail a patch keeps but the eye can skip, used to find
/// the one line of a hunk-less file that actually says something.
const DETAIL_MARKS: [&str; 3] = ["index ", "old mode ", "new mode "];

// ========================================================================
// Data Structures
// ========================================================================

/// One commit as the view shows it: the summary `git show` opens with, then each
/// file it touched.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommitContent {
    /// The summary lines above the first file: identity, author, dates, message,
    /// and the change totals, kept verbatim.
    pub summary: Vec<String>,
    /// Each file the commit touched, in the order git wrote them.
    pub files: Vec<CommitFile>,
}

/// One file's changes within a commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitFile {
    /// The path, taken from the new side of the `diff --git` line.
    pub path: String,
    /// What the commit did to it, as the status view's one-letter code.
    pub code: char,
    /// Its hunks, empty for a file with no textual diff.
    pub hunks: Vec<Hunk>,
    /// What git said instead of a diff, for a file that has none: the
    /// `Binary files … differ` note, or a bare mode change.
    pub note: Option<String>,
}

// ========================================================================
// CommitContent
// ========================================================================

impl CommitContent {
    /// Parse the lines of a `git show`.
    ///
    /// Everything before the first `diff --git` is the summary; each such line
    /// opens a file section that runs to the next one or to the end.
    pub fn parse(lines: &[String]) -> Self {
        let starts: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.starts_with(FILE_MARK))
            .map(|(index, _)| index)
            .collect();
        let summary_end = starts.first().copied().unwrap_or(lines.len());
        let mut files = Vec::with_capacity(starts.len());
        for (position, &start) in starts.iter().enumerate() {
            let end = starts.get(position + 1).copied().unwrap_or(lines.len());
            files.push(CommitFile::parse(&lines[start..end]));
        }
        Self {
            summary: lines[..summary_end].to_vec(),
            files,
        }
    }
}

// ========================================================================
// CommitFile
// ========================================================================

impl CommitFile {
    /// Parse one `diff --git` section.
    fn parse(section: &[String]) -> Self {
        let diff = parse_diff(&section.join("\n"));
        let hunks = diff.hunks;
        Self {
            path: file_path(section),
            code: file_code(section),
            note: hunks.is_empty().then(|| binary_note(section)).flatten(),
            hunks,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What a file's section did to it, as the status view's change code.
fn file_code(section: &[String]) -> char {
    if section.iter().any(|line| line.starts_with(NEW_FILE_MARK)) {
        'A'
    } else if section
        .iter()
        .any(|line| line.starts_with(DELETED_FILE_MARK))
    {
        'D'
    } else if section
        .iter()
        .any(|line| RENAME_MARKS.iter().any(|mark| line.starts_with(mark)))
    {
        'R'
    } else {
        'M'
    }
}

/// The path a file's section is about: the new side of its `diff --git` line.
fn file_path(section: &[String]) -> String {
    section
        .first()
        .and_then(|line| line.strip_prefix(FILE_MARK))
        .and_then(|rest| rest.split_once(" b/"))
        .map(|(_, path)| path.to_string())
        .unwrap_or_default()
}

/// The line of a hunk-less file's section that says what happened to it, if
/// there is one: `Binary files … differ`.
fn binary_note(section: &[String]) -> Option<String> {
    let detail = |line: &String| {
        line.starts_with(FILE_MARK)
            || line.starts_with("--- ")
            || line.starts_with("+++ ")
            || DETAIL_MARKS.iter().any(|mark| line.starts_with(mark))
    };
    section
        .iter()
        .skip(1)
        .find(|line| !detail(line))
        .filter(|line| !line.trim().is_empty())
        .cloned()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    fn sample() -> Vec<String> {
        shown(&[
            "commit abc1234567890",
            "Author: Someone <a@b.c>",
            "",
            "    do the thing",
            "",
            "diff --git a/kept.rs b/kept.rs",
            "index 1..2 100644",
            "--- a/kept.rs",
            "+++ b/kept.rs",
            "@@ -1 +1 @@",
            "-old",
            "+new",
            "diff --git a/added.txt b/added.txt",
            "new file mode 100644",
            "--- /dev/null",
            "+++ b/added.txt",
            "@@ -0,0 +1 @@",
            "+fresh",
        ])
    }

    #[test]
    fn test_the_summary_is_everything_before_the_first_file() {
        let content = CommitContent::parse(&sample());
        assert_eq!(content.summary.len(), 5);
        assert_eq!(content.summary[0], "commit abc1234567890");
        assert_eq!(content.summary[3], "    do the thing");
    }

    #[test]
    fn test_each_file_section_becomes_its_own_file() {
        let content = CommitContent::parse(&sample());
        assert_eq!(content.files.len(), 2);
        assert_eq!(content.files[0].path, "kept.rs");
        assert_eq!(content.files[0].code, 'M');
        assert_eq!(content.files[1].path, "added.txt");
        assert_eq!(
            content.files[1].code, 'A',
            "a new file mode makes it an addition"
        );
    }

    #[test]
    fn test_a_files_hunks_are_kept_so_they_can_fold_one_at_a_time() {
        let content = CommitContent::parse(&sample());
        assert_eq!(content.files[0].hunks.len(), 1);
        assert_eq!(content.files[0].hunks[0].lines, vec!["-old", "+new"]);
    }

    #[test]
    fn test_a_deleted_and_a_renamed_file_get_their_own_codes() {
        let deleted = CommitContent::parse(&shown(&[
            "diff --git a/gone.rs b/gone.rs",
            "deleted file mode 100644",
        ]));
        assert_eq!(deleted.files[0].code, 'D');
        let renamed = CommitContent::parse(&shown(&[
            "diff --git a/old.rs b/new.rs",
            "rename from old.rs",
            "rename to new.rs",
        ]));
        assert_eq!(renamed.files[0].code, 'R');
        assert_eq!(renamed.files[0].path, "new.rs");
    }

    #[test]
    fn test_a_binary_file_keeps_gits_note_instead_of_hunks() {
        let content = CommitContent::parse(&shown(&[
            "diff --git a/logo.png b/logo.png",
            "index 111..222 100644",
            "Binary files a/logo.png and b/logo.png differ",
        ]));
        assert!(content.files[0].hunks.is_empty());
        assert_eq!(
            content.files[0].note.as_deref(),
            Some("Binary files a/logo.png and b/logo.png differ")
        );
    }

    #[test]
    fn test_a_show_with_no_diff_is_all_summary() {
        let content = CommitContent::parse(&shown(&["commit abc", "", "    message"]));
        assert_eq!(content.summary.len(), 3);
        assert!(content.files.is_empty());
    }
}
