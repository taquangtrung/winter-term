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

/// The path a file's section is about, its new name where it has two: from
/// the `rename to` line, else the `+++ b/` side (`--- a/` for a deleted
/// file), else the `diff --git` line, whose two sides name one path when
/// nothing was renamed. Only the lines before the first hunk are read: a
/// removed line can start `--- ` too. Git writes a path holding a tab, a quote
/// or a newline in C quotes, and ends one holding a space with a tab.
fn file_path(section: &[String]) -> String {
    let header: Vec<&String> = section.iter().take_while(|line| !line.starts_with("@@")).collect();
    let line_of = |mark: &str| header.iter().find_map(|line| line.strip_prefix(mark));
    if let Some(to) = line_of(RENAME_MARKS[1]) {
        return unquote(to);
    }
    let side = |mark: &str, prefix: &str| {
        line_of(mark).and_then(|path| unquote(path.trim_end_matches('\t')).strip_prefix(prefix).map(str::to_string))
    };
    side("+++ ", "b/")
        .or_else(|| side("--- ", "a/"))
        .or_else(|| line_of(FILE_MARK).and_then(same_path))
        .unwrap_or_default()
}

/// The path both sides of a `diff --git` line name, `a/P b/P` or the same in
/// C quotes: read from its middle, so a path holding ` b/` itself is found.
fn same_path(sides: &str) -> Option<String> {
    let sides = sides.trim_end();
    let new = match sides.starts_with('"') {
        true => unquote(&sides[sides.rfind(" \"")? + 1..]),
        false => sides.get(sides.len().div_ceil(2)..)?.to_string(),
    };
    new.strip_prefix("b/").map(str::to_string)
}

/// `text` as git means it: C quotes taken off and their escapes read back
/// (`\t`, `\n`, `\"`, `\\`, and `\ooo` for each byte of a UTF-8 character),
/// or as it is when it is not quoted.
fn unquote(text: &str) -> String {
    let Some(inner) = text.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')) else {
        return text.to_string();
    };
    let mut bytes = Vec::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next() {
            Some('t') => bytes.push(b'\t'),
            Some('n') => bytes.push(b'\n'),
            Some(digit @ '0'..='7') => {
                let mut value = digit.to_digit(8).unwrap_or(0);
                for _ in 0..2 {
                    if let Some(next) = chars.peek().and_then(|c| c.to_digit(8)) {
                        value = value * 8 + next;
                        chars.next();
                    }
                }
                bytes.push(value as u8);
            }
            Some(other) => {
                let mut buf = [0; 4];
                bytes.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
            None => {}
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
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

    #[test]
    fn test_a_file_s_path_is_read_whole_whatever_it_holds() {
        // Split at the first ` b/`, a path holding one came out cut; a
        // quoted one kept its quotes; a renamed one is named by its new name.
        let lines = |text: &str| text.lines().map(str::to_string).collect::<Vec<_>>();
        let spaced = lines("diff --git a/x b/y b/z.txt b/x b/y b/z.txt\nindex 1..2 100644\n--- a/x b/y b/z.txt\t\n+++ b/x b/y b/z.txt\t\n@@ -1 +1,2 @@\n a\n+b");
        assert_eq!(file_path(&spaced), "x b/y b/z.txt");
        let quoted = lines("diff --git \"a/tab\\there\" \"b/tab\\there\"\n--- \"a/tab\\there\"\n+++ \"b/tab\\there\"\n@@ -1 +1 @@\n-q\n+qb");
        assert_eq!(file_path(&quoted), "tab\there");
        let renamed = lines("diff --git a/plain.txt b/moved name.txt\nold mode 100644\nnew mode 100755\nrename from plain.txt\nrename to moved name.txt");
        assert_eq!(file_path(&renamed), "moved name.txt");
        let deleted = lines("diff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n--- a removed line\n");
        assert_eq!(file_path(&deleted), "gone.txt");
        let mode_only = lines("diff --git a/x b/y.sh b/x b/y.sh\nold mode 100644\nnew mode 100755");
        assert_eq!(file_path(&mode_only), "x b/y.sh");
        assert_eq!(unquote("\"caf\\303\\251\""), "café", "octal bytes are read back as UTF-8");
    }

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
